use reliudp::SocketEvent;

fn print_values(received: &[u8]) {
    let chunks = received.chunks(16);
    for chunk in chunks {
        for v in chunk {
            print!("{:>3} ", v);
        }
        println!()
    }
}

fn main() -> Result<(), Box<dyn (::std::error::Error)>> {
    let ip = std::env::args().skip(1).next().unwrap_or(format!("127.0.0.1"));
    println!("Connecting to {}...", ip);
    let mut client = reliudp::RUdpSocket::connect((ip, 61243)).expect("Failed to create client");

    let mut received: Vec<u8> = vec!();
    for i in 0..2000 {
        client.next_tick()?;
        // if i % 10 == 0 { dbg!(client.status()); }
        for client_event in client.drain_events() {
            if let SocketEvent::Data(d) = client_event {
                let v = d.as_ref().get(0).unwrap();

                if received.contains(v) {
                    panic!("Value {} has already been received", v);
                } else {
                    received.push(*v);
                }
            } else {
                println!("Client: Incoming event {:?} at frame {:?}", client_event, i);
            }
        }
        
        ::std::thread::sleep(::std::time::Duration::from_millis(2));
        if received.len() >= 256 {
            println!("Finished! Values in order:");
            print_values(&received);
            return Ok(());
        }
    }
    println!("Not Finished... Values in order:");
    print_values(&received);
    println!();
    print!("Missing values:");
    for v in 0..=0xFF {
        if !received.contains(&v) {
            print!("{:>3} ", v);
        }
    }
    println!();
    Ok(())
}
