use std::sync::Arc;

fn generate_really_big_message(i: u8) -> Arc<[u8]> {
    let really_big_message: Vec<u8> = (0..2000).map(|_v| i).collect();
    let really_big_message: Arc<[u8]> = Arc::from(really_big_message.into_boxed_slice());
    really_big_message
}

fn main() -> Result<(), Box<dyn (::std::error::Error)>> {
    let mut server = reliudp::RUdpServer::new("0.0.0.0:61243").expect("Failed to create server");

    let mut can_start = false;
    let mut n = 0;
    for _ in 0usize.. {
        server.next_tick()?;
        for server_event in server.drain_events() {
            println!("Server: Incoming event {:?}", server_event);
            match server_event.1 {
                reliudp::SocketEvent::Connected => {
                    println!("Client connected! Starting.");
                    can_start = true
                },
                _ => {},
            }
        }

        if can_start {
            let big_message = generate_really_big_message(n);
            server.send_data(&big_message, reliudp::MessageType::KeyMessage);

            if n % 100 == 0 {
                for (address, socket) in server.iter() {
                    println!("\tPing to remote {:?} is {:?}", address, socket.ping());
                }
            }

            if n < 255 {
                n += 1;
            } else {
                println!("Finished sending data!");
                break;
            }
        }
        
        ::std::thread::sleep(::std::time::Duration::from_millis(5));
    }
    Ok(())
}