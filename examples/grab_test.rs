use std::time::{Duration, Instant};

fn main() {
    println!("Stasis grab test: will grab for 5 seconds, then auto-release.");
    println!("Press some keys (they won't reach other apps).");
    println!("CapsLock x3 should print 'UNLOCK MODE'.");
    println!();

    let (tx, rx) = crossbeam_channel::unbounded();
    let grab = stasis::platform::grab(tx).expect("grab failed");

    let start = Instant::now();
    let mut keypad = stasis::keypad::Keypad::new();

    while start.elapsed() < Duration::from_secs(5) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(stasis::platform::BackendEvent::Key(key)) => {
                let action = keypad.feed(Instant::now(), key);
                match action {
                    stasis::keypad::Action::UnlockMode => {
                        println!("[UNLOCK MODE] password len = 0");
                    }
                    stasis::keypad::Action::Password { len } => {
                        println!("[PWD] len = {}", len);
                    }
                    stasis::keypad::Action::Submit => {
                        println!("[SUBMIT] password = '{}'", keypad.password());
                        keypad.cancel_unlock_mode();
                    }
                    _ => {}
                }
            }
            Ok(stasis::platform::BackendEvent::ShiftRelease) => {
                keypad.shift_release();
            }
            Ok(stasis::platform::BackendEvent::Health(msg)) => {
                println!("[HEALTH] {}", msg);
            }
            Ok(stasis::platform::BackendEvent::Released) => {
                println!("[BACKEND RELEASED]");
                break;
            }
            Err(_) => {}
        }
    }

    drop(grab);
    println!("\nGrab released. Test complete.");
}
