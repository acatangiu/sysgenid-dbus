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
