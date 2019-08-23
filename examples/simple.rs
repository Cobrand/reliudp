extern crate reliudp;

use std::sync::Arc;

fn main() -> Result<(), Box<dyn (::std::error::Error)>> {
    let really_big_message: Vec<u8> = (0..65536).map(|v| (v % 256) as u8).collect();
    let really_big_message: Arc<[u8]> = Arc::from(really_big_message.into_boxed_slice());

    let mut server = reliudp::RUdpServer::new("0.0.0.0:50000").expect("Failed to create server");

    let mut client = reliudp::RUdpSocket::connect("192.168.1.89:50000").expect("Failed to create client");

    let mut sent_message: bool = false;

    println!("Created server & client. Starting main loop");
    for _frame in 0..300 {
        server.next_tick()?;
        client.next_tick()?;

        for client_event in client.drain_events() {
            println!("Client: Incoming event {:?}", client_event);
        }

        for server_event in server.drain_events() {
            println!("Server: Incoming event {:?}", server_event);
        }
        
        if !sent_message {
            client.send_data(Arc::clone(&really_big_message), reliudp::MessageType::KeyMessage);
            sent_message = true;
        }

        ::std::thread::sleep(::std::time::Duration::from_micros(16666));
    }

    {
        // drop the server;
        let _s = server;
    }
    
    for _frame in 0..10 {
        client.next_tick()?;

        for client_event in client.drain_events() {
            println!("Client: Incoming event {:?}", client_event);
        }
        ::std::thread::sleep(::std::time::Duration::from_micros(16666));
    }

    println!("Done.");
    Ok(())
}