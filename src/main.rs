use dbus::arg;
use dbus::blocking::Connection;
use dbus::channel::Sender;
use dbus::Message;
use dbus_crossroads::{Context, Crossroads, MethodErr};
use std::cmp::max;
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SYGENID_INTERFACE: &str = "com.RFC.sysgenid";
const SYGENID_PATH: &str = "/com/RFC/sysgenid";

struct Watcher {
    acked_gen_counter: u32,
}

struct Sysgenid {
    generation_counter: u32,
    watchers: HashMap<String, Watcher>,
    outdated_watchers: HashMap<String, Watcher>,
}

impl Sysgenid {
    pub fn new() -> Self {
        Sysgenid {
            generation_counter: 0,
            watchers: HashMap::new(),
            outdated_watchers: HashMap::new(),
        }
    }

    pub fn bump_generation(&mut self, ctx: &mut Context, min_gen: u32) {
        // Update generation counter.
        self.generation_counter = max(min_gen, self.generation_counter + 1);
        // TODO: update mapped value here
        // Signal watchers new generation event.
        let signal_msg = ctx.make_signal("NewGeneration", (self.generation_counter,));
        ctx.push_msg(signal_msg);
        // Mark all tracked watchers as outdated.
        self.outdated_watchers
            .extend(std::mem::take(&mut self.watchers));
    }

    pub fn update_watcher<F>(
        &mut self,
        watcher_id: &str,
        tracking: bool,
        watcher_counter: u32,
        signal_fn: F,
    ) -> Result<(), MethodErr>
    where
        F: FnOnce(&str),
    {
        if tracking {
            if watcher_counter != self.generation_counter {
                return Err(MethodErr::invalid_arg("watcher_counter"));
            }
            self.ack_watcher_gen_counter(watcher_id, signal_fn);
        } else {
            // Remove watcher from both tracking lists.
            self.watchers.remove(watcher_id);
            self.remove_outdated_watcher(watcher_id, signal_fn);
        }
        Ok(())
    }

    fn ack_watcher_gen_counter<F>(&mut self, watcher_id: &str, signal_fn: F)
    where
        F: FnOnce(&str),
    {
        if let Some(watcher) = self.watchers.get(watcher_id) {
            // Watcher is already up-to-date, but double check against program logic error.
            assert_eq!(watcher.acked_gen_counter, self.generation_counter);
        } else {
            // Watcher was either not tracked or outdated.
            // Start tracking as up-to-date watcher.
            self.watchers.insert(
                watcher_id.to_owned(),
                Watcher {
                    acked_gen_counter: self.generation_counter,
                },
            );
            self.remove_outdated_watcher(watcher_id, signal_fn);
        }
    }

    fn remove_outdated_watcher<F>(&mut self, watcher_id: &str, signal_fn: F)
    where
        F: FnOnce(&str),
    {
        if self.outdated_watchers.remove(watcher_id).is_some() && self.outdated_watchers.is_empty()
        {
            // Just removed the last outdated watcher; system is ready.
            signal_fn("SystemReady");
        }
    }
}

type LSysgenid = Arc<Mutex<Sysgenid>>;

#[derive(Debug)]
pub struct OrgFreedesktopDBusNameOwnerChanged {
    pub arg0: String,
    pub arg1: String,
    pub arg2: String,
}

impl arg::AppendAll for OrgFreedesktopDBusNameOwnerChanged {
    fn append(&self, i: &mut arg::IterAppend) {
        arg::RefArg::append(&self.arg0, i);
        arg::RefArg::append(&self.arg1, i);
        arg::RefArg::append(&self.arg2, i);
    }
}

impl arg::ReadAll for OrgFreedesktopDBusNameOwnerChanged {
    fn read(i: &mut arg::Iter) -> Result<Self, arg::TypeMismatchError> {
        Ok(OrgFreedesktopDBusNameOwnerChanged {
            arg0: i.read()?,
            arg1: i.read()?,
            arg2: i.read()?,
        })
    }
}

impl dbus::message::SignalArgs for OrgFreedesktopDBusNameOwnerChanged {
    const NAME: &'static str = "NameOwnerChanged";
    const INTERFACE: &'static str = "org.freedesktop.DBus";
}

fn main() -> Result<(), Box<dyn Error>> {
    let sysgenid = Arc::new(Mutex::new(Sysgenid::new()));

    // Start up a connection to the session bus and request a name.
    let c = Connection::new_session()?;
    c.request_name(SYGENID_INTERFACE, false, true, false)?;

    // Create a new crossroads instance so that introspection and properties interfaces
    // are added by default on object path additions.
    let mut cr = Crossroads::new();

    {
        let proxy = c.with_proxy(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            Duration::from_millis(5000),
        );
        let s2 = sysgenid.clone();
        // Let's start listening to signals.
        let _id = proxy.match_signal(
            move |h: OrgFreedesktopDBusNameOwnerChanged, c: &Connection, _: &Message| {
                // When there's someone leaving the bus,
                if h.arg0.eq(&h.arg1) {
                    let mut sysgenid = s2.lock().unwrap();
                    sysgenid
                        .update_watcher(&h.arg0, false, 0, |name| {
                            let mut signal_msg = dbus::Message::signal(
                                &SYGENID_PATH.into(),
                                &SYGENID_INTERFACE.into(),
                                &name.into(),
                            );
                            signal_msg.append_all(());
                            c.send(signal_msg).unwrap();
                        })
                        .unwrap();
                }
                true
            },
        );
    }

    // Let's build the interface.
    let iface_token = cr.register(SYGENID_INTERFACE, |b| {
        // This row is just for introspection: It advertises that we can send a
        // HelloHappened signal. We use the single-tuple to say that we have one single argument,
        // named "gen_counter" of type "u32".
        b.signal::<(u32,), _>("NewSystemGeneration", ("sysgen_counter",));
        b.signal::<(), _>("SystemReady", ());
        // Let's add a method to the interface. We have the method name, followed by
        // names of input and output arguments (used for introspection). The closure then controls
        // the types of these arguments. The last argument to the closure is a tuple of the input arguments.
        b.method(
            "GetSysGenCounter",
            (),
            ("sysgen_counter",),
            |_: &mut Context, data: &mut LSysgenid, ()| {
                let sysgenid = data.lock().unwrap();
                Ok((sysgenid.generation_counter,))
            },
        );
        b.method(
            "CountOutdatedWatchers",
            (),
            ("outdated_watchers",),
            |_: &mut Context, data: &mut LSysgenid, ()| {
                let sysgenid = data.lock().unwrap();
                let ret = sysgenid.outdated_watchers.len() as u32;
                Ok((ret,))
            },
        );
        b.method(
            "UpdateWatcher",
            ("tracking_enabled", "watcher_counter"),
            ("sysgen_counter",),
            |ctx: &mut Context, data: &mut LSysgenid, (tracking, watcher_counter): (bool, u32)| {
                let watcher_id = ctx
                    .message()
                    .sender()
                    .ok_or(MethodErr::failed("could not identify sender"))?
                    .to_string();
                let mut sysgenid = data.lock().unwrap();
                sysgenid.update_watcher(&watcher_id, tracking, watcher_counter, |name| {
                    let signal_msg = ctx.make_signal(name, ());
                    ctx.push_msg(signal_msg);
                })?;
                Ok((sysgenid.generation_counter,))
            },
        );
        b.method(
            "TriggerSysGenUpdate",
            ("min_gen",),
            (),
            |ctx: &mut Context, data: &mut LSysgenid, (min_gen,): (u32,)| {
                let mut sysgenid = data.lock().unwrap();
                sysgenid.bump_generation(ctx, min_gen);
                Ok(())
            },
        );
    });

    // Let's add the "/" path, which implements the com.RFC.sysgenid interface.
    cr.insert(SYGENID_PATH, &[iface_token], sysgenid);

    // Serve clients forever.
    cr.serve(&c)?;
    unreachable!()
}
