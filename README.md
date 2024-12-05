# mcutie

[![Tests](https://github.com/Mossop/mcutie/actions/workflows/test.yml/badge.svg)](https://github.com/Mossop/mcutie/actions/workflows/test.yml)
[![Latest Version](https://img.shields.io/crates/v/mcutie.svg)](https://crates.io/crates/mcutie)
[![Documentation](https://docs.rs/mcutie/badge.svg)](https://docs.rs/mcutie)

A simple MQTT client designed for use in embedded devices using the `embassy-net` networking stack.
Requires an async executor. Runs in `no-std` contexts and includes specific support for Home
Assistant's device auto-discovery.

The stack is designed to be a singleton for the lifetime of the application. Once initialised it
runs forever attempting to maintain a connection to the MQTT broker. It includes support for
automatically subscribing to some topics when connected to the broker and registering a last will
message to be published if the connection to the broker it lost.

Has some basic support for the different MQTT QoS levels.
