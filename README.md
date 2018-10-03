# Reliudp: Custom reliable udp protocol for Rust

(Work in progress)

## Short version

Features:

* No unsafe code
* All the downsides of UDP with the sluggishness of TCP
* Slow as a camel
* I'm not sure how it works
* Experimental
* Not safe against DOS attacks

Downsides:

* It works

## Long version

This crate is meant be be akin to `https://github.com/BonsaiDen/cobalt-rs`, in the sense that
it provides:

* Connection over UDP
* Guarantee that a message arrives at destination
* Extremely Low Latency

This crate was mainly made in mind for use cases where *bandwidth is plentifull* and
*low latency is of the utmost importance*. Specifically, online multiplayer gaming.

### What is the advantage over raw UDP?

* Keeps track of connection, handles timeouts (uses 2-way handshake)
* Automatic packet re-ordering
* Additional CRC32 check over IP packets, reducing chances of corrupted data to almost none
* Possibility to make sure a packet arrives at destination

### What is the advantage over raw TCP?

* Not stream based
* You choose which packets should be discarded if it doesn't go through the first time:
    * Key messages will make sure to go through with an Ack system
    * Forgettable messages will be discarded if packet wasn't complete the first time

### What are the drawbacks ?

* (Planned) No network congestion handling
* (Planned) No test coverage
* (Planned) No optionnal SSL and/or other secure way to send data: everything is plaintext
* (Planned) No error correction codes of any kind (Hamming, ...)
* (May change in the future) Payloads are limited in size (around ~300KB)
* Untested against DOS attacks
* No `Future`s support. Will probably never have it.

## License

MIT