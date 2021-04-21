use std::time::Duration;

use dbus::message::MatchRule;
use dbus::nonblock;
use dbus::nonblock::SyncConnection;
use dbus_tokio::connection;
use std::sync::{Arc, Mutex};
use tokio;
use uuid::Uuid;

/// This example shows an `Application` doing some app-specific periodic work,
/// while also listening for SysGenID events. On receipt of a system generation
/// change signal, it will adjust to new generation, acknowledge it back to the
/// server and continue work.

const SYSGENID_INTERFACE: &str = "com.RFC.sysgenid";
const SYGENID_PATH: &str = "/com/RFC/sysgenid";

pub struct Application {
    // Internal unique data that we want to change on each system generation bump.
    uuid: Uuid,
    // Flag that shows when running dirty (with old generation data).
    dirty_uniqueness: bool,

    // Connection to SysGenID DBus server.
    conn: Arc<SyncConnection>,
    // Whether this Application wants to be tracked by SysGenID server.
    // Tracked clients are expected to explicitly acknowledge back to the server
    // when they have adjusted to a new generation.
    tracking_enabled: bool,
}

impl Application {
    /// This is the `Application` main function, where it does its specific work.
    /// In this example it only prints something.
    pub async fn main_loop(app_lock: Arc<Mutex<Self>>) {
        // Create interval - a Stream that will fire an event periodically
        // to simulate periodic events that the application handles.
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        // This would be a simple main loop.
        loop {
            interval.tick().await;
            let mut app = app_lock.lock().unwrap();
            if app.dirty_uniqueness {
                app.adjust_to_new_generation().await;
                println!("Client: adjusted, continuing workload...");
            }
            // In a real app this would be main loop event handler.
            // In this example, we just call this every 2 seconds.
            app.do_app_specific_work();
        }
    }

    fn do_app_specific_work(&self) {
        println!(
            "Client: example doing some periodic work (uuid {})",
            self.uuid
        );
    }

    pub fn new_generation_handler(&mut self) {
        println!("Client: got NewGeneration signal! Marking dirty...");
        self.dirty_uniqueness = true;
    }

    async fn adjust_to_new_generation(&mut self) {
        let proxy = nonblock::Proxy::new(
            SYSGENID_INTERFACE,
            SYGENID_PATH,
            Duration::from_secs(2),
            self.conn.clone(),
        );

        println!("Client: getting new generation (using DBus method GetSysGenCounter)...");
        let (counter,): (u32,) = proxy
            .method_call(SYSGENID_INTERFACE, "GetSysGenCounter", ())
            .await
            .unwrap();
        println!("Client: got new gen counter: {}", counter);

        println!("Client: adjusting to new environment...");
        self.uuid = Uuid::new_v4();
        self.dirty_uniqueness = false;
        println!(
            "Client: adjusted to new environment: new UUID: {}",
            self.uuid
        );

        if self.tracking_enabled {
            println!(
                "Client: acknowledging adjustment complete (using DBus method AckWatcherCounter)..."
            );
            let (counter,): (u32,) = proxy
                .method_call(SYSGENID_INTERFACE, "AckWatcherCounter", (counter,))
                .await
                .unwrap();
            println!("Client: acknowledged new counter: {}", counter);
        }
    }

    pub fn new(conn: Arc<SyncConnection>, tracking_enabled: bool) -> Self {
        Application {
            uuid: Uuid::new_v4(),
            dirty_uniqueness: false,
            conn,
            tracking_enabled,
        }
    }
}

pub fn new_untracked_app(conn: Arc<SyncConnection>) -> Application {
    Application::new(conn, false)
}

pub async fn new_tracked_app(conn: Arc<SyncConnection>) -> Application {
    // Ping SysGenID service so it starts tracking this client.
    let proxy = nonblock::Proxy::new(
        SYSGENID_INTERFACE,
        SYGENID_PATH,
        Duration::from_secs(2),
        conn.clone(),
    );
    let (counter,): (u32,) = proxy
        .method_call(SYSGENID_INTERFACE, "GetSysGenCounter", ())
        .await
        .unwrap();
    let (_,): (u32,) = proxy
        .method_call(SYSGENID_INTERFACE, "AckWatcherCounter", (counter,))
        .await
        .unwrap();

    Application::new(conn, true)
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

    // Create `Application` client with tracking enabled.
    let app = Arc::new(Mutex::new(new_tracked_app(conn.clone()).await));

    // To receive D-Bus signals we need to add a match that defines which signals should be forwarded
    // to our application.
    let app2 = app.clone();
    let mr = MatchRule::new_signal(SYSGENID_INTERFACE, "NewGeneration");
    let incoming_signal = conn.add_match(mr).await?.cb(move |_, (_counter,): (u32,)| {
        app2.lock().unwrap().new_generation_handler();
        true
    });

    // This will never return (except on panic) as there's no exit condition in do_work().
    Application::main_loop(app).await;

    // Needed here to ensure the "incoming_signal" object is not dropped too early
    conn.remove_match(incoming_signal.token()).await?;

    unreachable!()
}
