use ::std::time::Instant;

#[derive(Debug)]
pub (crate) struct PingHandler {
    pub waiting_ping: Option<(u32, Instant)>,
    // in ms
    pub current_ping: Option<u32>,
}

impl PingHandler {
    pub fn new() -> PingHandler {
        PingHandler {
            waiting_ping: None,
            current_ping: None
        }
    }

    /// Should be called when we send the packet that will act as a ping
    ///
    /// Does nothing if there is already another last_ping_sent recorded unanswered
    pub (crate) fn ping(&mut self, seq_id: u32) {
        let now = Instant::now();
        let delta_sec = self.waiting_ping.map(|(_, time)| {
            (now - time).as_secs()
        });
        if let Some(delta_sec) = delta_sec {
            if delta_sec >= 5 {
                // if we haven't received an answer to our ping after 5s, we'll assume he never
                // received it and we will send another one
                self.waiting_ping = None;
            } else {
                // current ping is valid, we will skip storing given seq_id
                return;
            }
        }
        self.waiting_ping = Some((seq_id, now));
    }

    /// Should be called when we receive the ping back
    ///
    /// Does nothing if the seq_id has not been recorded
    pub (crate) fn pong(&mut self, seq_id: u32) {
        let clear_waiting_ping: bool = match self.waiting_ping {
            Some((stored_seq_id, time)) if stored_seq_id == seq_id => {
                let d = Instant::now() - time;
                let ms = d.subsec_millis();
                let secs = d.as_secs();
                let ping_ms = if secs >= 5 {
                    4999u32
                } else {
                    ms + (secs as u32) * 1000
                };
                self.current_ping = Some(ping_ms);
                true
            },
            _ => false
        };
        if clear_waiting_ping {
            self.waiting_ping = None;
        }
    }

    /// Returns the current ping is ms. Returns None if ping wasn't computed already
    pub (crate) fn current_ping_ms(&self) -> Option<u32> {
        self.current_ping
    }
}