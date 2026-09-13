use std::time::{Duration, Instant};

fn main() {
    eprintln!("Stasis unlock test: input grabbed NOW.");
    eprintln!("1. Press CapsLock 3 times within 2 seconds");
    eprintln!("2. Type password: 123456");
    eprintln!("3. Press Enter");
    eprintln!("Auto-releases after 240s. Kill with: pkill -f unlock_test");
    eprintln!();

    let (tx, rx) = crossbeam_channel::unbounded();
    let grab = stasis::platform::grab(tx).expect("grab failed");
    eprintln!("[LOCKED] Input grabbed successfully.");

    let start = Instant::now();
    let mut keypad = stasis::keypad::Keypad::new();
    let mut unlocked = false;

    while start.elapsed() < Duration::from_secs(240) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(stasis::platform::BackendEvent::Key(key)) => {
                let action = keypad.feed(Instant::now(), key);
                match action {
                    stasis::keypad::Action::UnlockMode => {
                        eprintln!("[UNLOCK MODE] Enter password...");
                    }
                    stasis::keypad::Action::Password { len } => {
                        eprintln!("[PWD] {} chars entered", len);
                    }
                    stasis::keypad::Action::Submit => {
                        let pwd = keypad.password();
                        eprintln!("[SUBMIT] checking '{}'", pwd);
                        if pwd == "123456" {
                            eprintln!("[CORRECT] Unlocking!");
                            unlocked = true;
                            break;
                        } else {
                            eprintln!("[WRONG] Try again: CapsLock x3 then password.");
                            keypad.cancel_unlock_mode();
                        }
                    }
                    _ => {}
                }
            }
            Ok(stasis::platform::BackendEvent::ShiftRelease) => {
                keypad.shift_release();
            }
            Ok(stasis::platform::BackendEvent::Health(msg)) => {
                eprintln!("[HEALTH] {}", msg);
            }
            Ok(stasis::platform::BackendEvent::Released) => {
                eprintln!("[BACKEND RELEASED unexpectedly]");
                break;
            }
            Err(_) => {}
        }
    }

    drop(grab);
    if unlocked {
        eprintln!("\nRESULT: PASS - unlock flow works.");
    } else {
        eprintln!("\nRESULT: TIMEOUT - no correct password within 240s.");
    }
}
