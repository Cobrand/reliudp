use std::sync::Arc;

fn generate_really_big_message(i: u8) -> Arc<[u8]> {
    let really_big_message: Vec<u8> = (0..2000).map(|_v| i).collect();
    let really_big_message: Arc<[u8]> = Arc::from(really_big_message.into_boxed_slice());
    really_big_message
}

fn main() -> Result<(), Box<dyn (::std::error::Error)>> {
    let mut server = reliudp::RUdpServer::new("0.0.0.0:61244").expect("Failed to create server");

    let mut n = 0;
    for i in 0u64.. {
        server.next_tick()?;
        for server_event in server.drain_events() {
            println!("Server: Incoming event {:?}", server_event);
        }

        if i % 300 == 0 && server.remotes_len() > 0 {
            let big_message = generate_really_big_message(n);
            println!("Sending (n={:?}) {:?} bytes to all {:?} remotes", n, big_message.as_ref().len(), server.remotes_len());
            if n % 2 == 0 {
                server.send_data(&big_message, reliudp::MessageType::KeyMessage, Default::default());
            } else {
                server.send_data(&big_message, reliudp::MessageType::Forgettable, Default::default());
            }
            n += 1;

            for (address, socket) in server.iter() {
                println!("\tPing to remote {:?} is {:?}", address, socket.ping());
            }
        }
        
        ::std::thread::sleep(::std::time::Duration::from_millis(5));
    }
    Ok(())
}