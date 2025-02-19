use reliudp::SocketEvent;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = reliudp::RUdpSocket::connect("127.0.0.1:61244").expect("Failed to create client");
    for i in 0.. {
        client.next_tick()?;
        // if i % 10 == 0 { dbg!(client.status()); }
        for client_event in client.drain_events() {
            if let SocketEvent::Data(d) = client_event {
                println!("Client: Incoming {:?} bytes (n={:?}) at frame {:?}", d.len(), d[0], i);
            } else {
                println!("Client: Incoming event {:?} at frame {:?}", client_event, i);
            }
        }
        for (remote, data) in client.drain_unknown() {
            println!("Client: message from unknown remote {}: '{}'", remote, String::from_utf8_lossy(&data));
        }
        
        std::thread::sleep(::std::time::Duration::from_millis(5));
    }
    Ok(())
}
