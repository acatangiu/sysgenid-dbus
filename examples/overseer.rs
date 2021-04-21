use std::time::Duration;

use dbus::message::MatchRule;
use dbus::nonblock;
use dbus::nonblock::{MsgMatch, SyncConnection};
use dbus_tokio::connection;
use std::sync::{Arc, Mutex};
use tokio;

/// This example shows a simple `Overseer`-type application.
/// IRL such an app would:
/// 1. quiesce the system (turn off networking for example) before a snapshot happens,
/// 2. bump sys gen id after system is loaded from snapshot,
/// 3. wait for all consumer apps to readjust to the new environment (wait for SystemReady signal),
/// 4. un-quiesce system (rollback step 1) bringing it back to active state.

const SYSGENID_INTERFACE: &str = "com.RFC.sysgenid";
const SYGENID_PATH: &str = "/com/RFC/sysgenid";

#[derive(PartialEq)]
enum SystemState {
    Quiescing,
    Quiesced,
    Adjusting,
    Adjusted,
    Unquiescing,
    Ready,
}

struct Overseer {
    system_state: SystemState,
    // Connection to SysGenID DBus server.
    conn: Arc<SyncConnection>,
}

impl Overseer {
    pub fn new(conn: Arc<SyncConnection>) -> Self {
        Overseer {
            system_state: SystemState::Ready,
            conn,
        }
    }

    // Easy way to share state across async handlers is wrapping in Arc<Mutex>,
    // a production level application would probably spawn a dedicated async task
    // to manage state and would use message passing between tasks to operate on it.
    pub async fn register_system_ready_handler(ovs: Arc<Mutex<Self>>) -> MsgMatch {
        let ovs2 = ovs.clone();
        let mr = MatchRule::new_signal(SYSGENID_INTERFACE, "SystemReady");
        let conn = ovs.lock().unwrap().conn.clone();
        conn.add_match(mr).await.unwrap().cb(move |_, (): ()| {
            ovs2.lock().unwrap().system_adjusted_signal_handler();
            true
        })
    }

    pub fn quiesce(&mut self) {
        self.system_state = SystemState::Quiescing;
        // Do actual quiescing instead of simple print.
        println!("Overseer: do quiesce.");
        self.system_state = SystemState::Quiesced;
    }

    pub async fn bump_generation(&mut self) {
        let proxy = nonblock::Proxy::new(
            SYSGENID_INTERFACE,
            SYGENID_PATH,
            Duration::from_secs(2),
            self.conn.clone(),
        );
        println!("Overseer: trigger new generation (min gen counter 0)!");
        let (): () = proxy
            .method_call(SYSGENID_INTERFACE, "TriggerSysGenUpdate", (0 as u32,))
            .await
            .unwrap();
    }

    pub async fn count_outdated_watchers(&self) -> u32 {
        let proxy = nonblock::Proxy::new(
            SYSGENID_INTERFACE,
            SYGENID_PATH,
            Duration::from_secs(2),
            self.conn.clone(),
        );
        println!("Overseer: call 'CountOutdatedWatchers'");
        let (count,): (u32,) = proxy
            .method_call(SYSGENID_INTERFACE, "CountOutdatedWatchers", ())
            .await
            .unwrap();
        println!("Overseer: 'CountOutdatedWatchers' method result {}", count);
        count
    }

    pub async fn wait_system_adjust(ovs: Arc<Mutex<Self>>) {
        ovs.lock().unwrap().system_state = SystemState::Adjusting;

        // Check if there are any outdated watchers to wait for.
        let outdated_watchers = ovs.lock().unwrap().count_outdated_watchers().await;
        if outdated_watchers != 0 {
            println!(
                "Overseer: There are {} outdated watchers across the system. Waiting for them...",
                outdated_watchers
            );
            // IRL this would be conditional variable or something other than polling.
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            while ovs.lock().unwrap().system_state != SystemState::Adjusted {
                interval.tick().await;
            }
        } else {
            println!("Overseer: There are no outdated watchers across the system. Moving on.");
            ovs.lock().unwrap().system_state = SystemState::Adjusted;
        }
    }

    fn system_adjusted_signal_handler(&mut self) {
        println!("Overseer: System is adjusted (got SystemReady DBus signal)!");
        self.system_state = SystemState::Adjusted;
    }

    pub fn unquiesce(&mut self) {
        self.system_state = SystemState::Unquiescing;
        // Do actual unquiescing.
        println!("Overseer: Overseer do un-quiesce.");
        self.system_state = SystemState::Ready;
        println!("Overseer: System ready!");
    }
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the D-Bus session bus (this is blocking, unfortunately).
    let (resource, conn) = connection::new_session_sync()?;

    // The resource is a task that should be spawned onto a tokio compatible
    // reactor ASAP. If the resource ever finishes, you lost connection to D-Bus.
    tokio::spawn(async {
        let err = resource.await;
        panic!("Lost connection to D-Bus: {}", err);
    });

    // Create `Overseer`.
    let ovs = Arc::new(Mutex::new(Overseer::new(conn.clone())));
    // Register handler for SystemReady signal.
    let incoming_signal = Overseer::register_system_ready_handler(ovs.clone()).await;

    ovs.lock().unwrap().quiesce();
    ovs.lock().unwrap().bump_generation().await;
    Overseer::wait_system_adjust(ovs.clone()).await;
    ovs.lock().unwrap().unquiesce();

    // Needed here to ensure the "incoming_signal" object is not dropped too early
    conn.remove_match(incoming_signal.token()).await?;

    Ok(())
}
