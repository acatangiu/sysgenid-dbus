# SysGenID: a system generation id provider

## Background and problem

The System Generation ID feature is required in virtualized or
containerized environments by applications that work with local copies
or caches of world-unique data such as random values, uuids,
monotonically increasing counters, cryptographic nonces, etc.
Such applications can be negatively affected by VM or container
snapshotting when the VM or container is either cloned or returned to
an earlier point in time.

Solving the uniqueness problem strongly enough for cryptographic
purposes requires a mechanism which can deterministically reseed
userspace PRNGs with new entropy at restore time. This mechanism must
also support the high-throughput and low-latency use-cases that led
programmers to pick a userspace PRNG in the first place; be usable by
both application code and libraries; allow transparent retrofitting
behind existing popular PRNG interfaces without changing application
code; it must be efficient, especially on snapshot restore; and be
simple enough for wide adoption.

## Solution

Introduce a mechanism that standardizes an API for
applications and libraries to be made aware of uniqueness breaking
events such as VM or container snapshotting, and allow them to react
and adapt to such events.

The System Generation ID is meant to help in these scenarios by
providing a monotonically increasing u32 counter that changes each time
the VM or container is restored from a snapshot.

The `sysgenid` service exposes a monotonic incremental System Generation
u32 counter via the DBus `com.RFC.sysgenid` accessible at
`/com/RFC/sysgenid`. It provides asynchronous SysGen
counter update notifications, as well as counter retrieval and
confirmation mechanisms.
The counter starts from zero when the service is started and
monotonically increments every time the system generation changes.

Userspace applications or libraries can (a)synchronously consume the
system generation counter through the provided DBus interface, to
make any necessary internal adjustments following a system generation
update.

The provided DBus interface operations can be used to build a
system level safe workflow that guest software can follow to protect
itself from negative system snapshot effects.

System generation changes are driven by userspace software through a
dedicated DBus method.

### Warning
SysGenID alone does not guarantee complete snapshot
safety to applications using it. A certain workflow needs to be
followed at the system level, in order to make the system
snapshot-resilient. Please see the "Snapshot Safety Prerequisites"
section below.

## SysGenID DBus interface

#### Terminology
 - `watcher` - a client using the SysGenID service _watching_ for system generation changes.
 - `untracked watcher` - default state for all clients. For a client to be tracked it has
   to explicitly opt-in by confirming back to the service the correct _system generation
   counter_.
 - `tracked watcher` - a client that is tracked by the service. Such a watcher is considered
   `up-to-date` only after confirming back to the service the correct
   _system generation counter_.
   Once tracked, a client is only _untracked_ when closing its connection to the DBus bus.
 - `outdated watcher` - a _tracked_ client that whose tracking has lived through a system
   generation change, but has not (yet) confirmed back to the service the correct _system
   generation counter_.

**Methods:**
- `GetSysGenCounter` - returns latest system generation counter.
- `AckWatcherCounter` - marks the client/watcher to be tracked for ACKs, is also
  used by the watcher to confirm/ack the correct _sys gen counter_ to the service after
  every generation change so the service keeps correct track of it as `outdated` or
  `up-to-date`.
  Will error if client/watcher confirms/acks the wrong _sys gen counter_.
- `CountOutdatedWatchers` - returns the number of current number of
  _outdated tracked watchers_.
  A value of `zero` can be interpreted as the system being fully re-adjusted after a
  generation change.
- `TriggerSysGenUpdate` - triggers a generation update (should be a privileged operation).

**Signals:**
- `NewSystemGeneration` - system generation change notification, also carries new
  _sys gen counter_.
- `SystemReady` - notification sent out when all tracked watchers have _acked_ the new
  _sys gen counter_. In other words, when all tracked software has adjusted to the new
  environment.

The service can keep track of watchers by DBus connections
(`org.freedesktop.DBus.NameOwnerChanged`).

**Exported read-only file used for memory mappings:**

The service also exports the current _sys gen counter_ through a simple file.
The file contains only 4 bytes of data at offset 0, representing the u32 value
of the system generation counter.
This file is meant to be mapped by other software in the system and be used as
a low-latency generation counter probe mechanism in critical sections.
This mmap() interface is targeted at libraries or code that needs to
check for generation changes in-line, where an event loop is not
available or in cases where DBus calls are too expensive.
In such cases, logic can be added in-line with the sensitive code to check the
counter and trigger on-demand/just-in-time readjustments when changes are
detected on the memory mapped file.

Users of this interface that plan to lazily adjust most likely don't need to
also use the DBus interface, since tracking or waiting on them doesn't make sense.

### Service interface DBus XML specification
```xml
<node name="/com/RFC/sysgenid">
  <interface name="com.RFC.sysgenid">
    <method name="AckWatcherCounter">
      <arg name="watcher_counter" type="u" direction="in"/>
      <arg name="sysgen_counter" type="u" direction="out"/>
    </method>
    <method name="CountOutdatedWatchers">
      <arg name="outdated_watchers" type="u" direction="out"/>
    </method>
    <method name="GetSysGenCounter">
      <arg name="sysgen_counter" type="u" direction="out"/>
    </method>
    <method name="TriggerSysGenUpdate">
      <arg name="min_gen" type="u" direction="in"/>
    </method>
    <signal name="NewSystemGeneration">
      <arg name="sysgen_counter" type="u"/>
    </signal>
    <signal name="SystemReady">
    </signal>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect">
      <arg name="xml_data" type="s" direction="out"/>
    </method>
  </interface>
</node>
```

## Snapshot Safety Prerequisites and Example

If VM, container or other system-level snapshots happen asynchronously,
at arbitrary times during an active workload there is no practical way
to ensure that in-flight local copies or caches of world-unique data
such as random values, secrets, UUIDs, etc are properly scrubbed and
regenerated.
The challenge stems from the fact that the categorization of data as
snapshot-sensitive is only known to the software working with it, and
this software has no logical control over the moment in time when an
external system snapshot occurs.

Let's take an OpenSSL session token for example. Even if the library
code is made 100% snapshot-safe, meaning the library guarantees that
the session token is unique (any snapshot that happened during the
library call did not duplicate or leak the token), the token is still
vulnerable to snapshot events while it transits the various layers of
the library caller, then the various layers of the OS before leaving
the system.

To catch a secret while it's in-flight, we'd have to validate system
generation at every layer, every step of the way. Even if that would
be deemed the right solution, it would be a long road and a whole
universe to patch before we get there.

Bottom line is we don't have a way to track all of these in-flight
secrets and dynamically scrub them from existence with snapshot
events happening arbitrarily.

### Simplifying assumption - safety prerequisite

**Control the snapshot flow**, disallow snapshots coming at arbitrary
moments in the workload lifetime.

Use a system-level overseer entity that quiesces the system before
snapshot, and post-snapshot-resume oversees that software components
have readjusted to new environment, to the new generation. Only after,
will the overseer un-quiesce the system and allow active workloads.

Software components can choose whether they want to be tracked and
waited on by the overseer by using the marking themselves as tracked
watchers.

The sysgenid service standardizes the API for system software to
find out about needing to readjust and at the same time provides a
mechanism for the overseer entity to wait for everyone to be done, the
system to have readjusted, so it can un-quiesce.

### Example snapshot-safe workflow

1) Before taking a snapshot, quiesce the VM/container/system. Exactly
   how this is achieved is very workload-specific, but the general
   description is to get all software to an expected state where their
   event loops dry up and they are effectively quiesced.
2) Take snapshot.
3) Resume the VM/container/system from said snapshot.
4) Overseer will trigger generation bump using
   `TriggerSysGenUpdate` method.
5) Software components which have the DBus `NewGeneration` signal in
   their event loops are notified of the generation change.
   They do their specific internal adjustments. Some may have chosen to
   be tracked and waited on by the overseer, others might choose to do
   their adjustments out of band and not block the overseer.
   Tracked ones *must* signal when they are done/ready by confirming the
   new sys gen counter using the `AckWatcherCounter` DBus method.
6) Overseer will block and wait for all tracked watchers by waiting on
   the `SystemReady` DBus signal. Once all tracked watchers are done
   in step 5, the signal is sent by `sysgenid` service and overseer will
   know that the system has readjusted and is ready for active workload.
7) Overseer un-quiesces system.
8) There is a class of software, usually libraries, most notably PRNGs
   or SSLs, that don't fit the event-loop model and also have strict
   latency requirements. These can take advantage of the
   _exported read-only file used for memory mappings_. They can map the
   file and check sys gen counter value in-line with the critical section
   and can do so with low latency. When they are called after un-quiesce,
   they can just-in-time adjust based on the updated mapped value.
   For a well-designed service stack, these libraries should not be
   called while system is quiesced. When workload is resumed by the
   overseer, on the first call into these libs, they will safely JIT
   readjust.
   Users of this lazy on-demand readjustment model should not use the
   DBus interface or at least not enable watcher tracking since doing so
   would introduce a logical deadlock:
   lazy adjustments happen only after un-quiesce, but un-quiesce is
   blocked until all tracked watchers are up-to-date.

## Provided code examples

The repo contains two code examples `examples/client.rs` and
`examples/overseer.rs` that showcase the SysGenID service capabilities
and provide a model for using this service.

`client.rs` - shows an _Application_ doing some app-specific periodic work,
while also listening for SysGenID events. On receipt of a system generation
change signal, it will adjust to new generation, acknowledge it back to the
service and continue work.

`overseer.rs` - shows shows a simple _Overseer-type_ application. This simple
implementation goes through the following steps then exits:
1. quiesces the system (IRL turn off networking for example - this example
   only prints a message) before a snapshot happens,
2. bumps sys gen id after system is loaded from snapshot,
3. waits for all consumer apps to readjust to the new environment (waits
   for `SystemReady` signal),
4. un-quiesce system (IRL rollback step 1 - this example only prints message)
   bringing it back to active state.

The whole SysGenID dance can be exercised by running the service, running
one or more instances of `examples/client`, then running `examples/overseer`.

### Example run
`example_run.sh` code:
```bash
#!/bin/bash

# Build service
cargo +stable build
# Build examples
cargo +stable build --examples

# Kill old instances of SysGenID DBus service
killall sysgenid-dbus
# Start new instance of SysGenID DBus service
cargo +stable run &

# Give the service a chance to start
sleep 1

# Run a client instance in the background
cargo +stable run --example client &
CLIENT_PID=$!

# Give it a few seconds of peace and quiet
# This is it running before snapshot
sleep 4

# Run the overseer example which would run
# right after the snapshot. Overseer exits by itself
cargo +stable run --example overseer

# Give the client a few more seconds of
# simulated post-snapshot work
sleep 4

# Kill the client
kill $CLIENT_PID

# Kill SysGenID DBus service
killall sysgenid-dbus
```
`example_run.sh` output:
```bash
sysgenid-dbus$ ./run_examples.sh

Client: example doing some periodic work (uuid 9354901b-1af1-4ef0-b19b-6901cd09d925)
Client: example doing some periodic work (uuid 9354901b-1af1-4ef0-b19b-6901cd09d925)
Client: example doing some periodic work (uuid 9354901b-1af1-4ef0-b19b-6901cd09d925)
Overseer: do quiesce.
Overseer: trigger new generation (min gen counter 0)!
Overseer: call 'CountOutdatedWatchers'
Client: got 'NewGeneration' signal! Marking dirty...
Overseer: 'CountOutdatedWatchers' method result 1
Overseer: There are 1 outdated watchers across the system. Waiting for them...
Client: getting new generation (using DBus method 'GetSysGenCounter')...
Client: got new gen counter: 1
Client: adjusting to new environment...
Client: adjusted to new environment: new UUID: 2c11d75a-a654-44b6-bbaf-c740b9a5b161
Client: acknowledging adjustment complete (using DBus method 'AckWatcherCounter')...
Client: acknowledged new counter: 1
Client: adjusted, continuing workload...
Client: example doing some periodic work (uuid 2c11d75a-a654-44b6-bbaf-c740b9a5b161)
Overseer: System is adjusted (got 'SystemReady' DBus signal)!
Overseer: Overseer do un-quiesce.
Overseer: System ready!
Client: example doing some periodic work (uuid 2c11d75a-a654-44b6-bbaf-c740b9a5b161)
Client: example doing some periodic work (uuid 2c11d75a-a654-44b6-bbaf-c740b9a5b161)

./run_examples.sh: line 36:  1269 Terminated              cargo +stable run
./run_examples.sh: line 36:  1272 Terminated              cargo +stable run --example client
```
