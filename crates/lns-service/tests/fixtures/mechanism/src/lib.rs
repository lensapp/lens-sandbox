//! One mechanism per feature, each doing the least that proves one bound.
//!
//! These stand in for a real connector's implementation. lns cannot read what a
//! component does, so what the tests need is a component that does one legible
//! thing and reports which way the host answered it.
//!
//! One build compiles one feature of the twenty-three, so what the other
//! twenty-two use is dead in each of them — and `make lint` builds them all.
#![allow(unused_imports, dead_code)]

wit_bindgen::generate!({ world: "mechanism", path: "../../../wit" });

use exports::lns::connector::adapter::Guest;
use lns::connector::types::{Answer, Ask, CallError, Field, Outcome, Step};

struct Fixture;

fn answered(name: &str, value: &str) -> Step {
    Step::Done(Outcome {
        values: vec![Answer {
            name: name.to_string(),
            value: value.to_string(),
        }],
        authority: vec!["read".to_string()],
        expires_at_millis: None,
    })
}

fn how_it_was_refused(error: &CallError) -> Step {
    match error {
        CallError::Refused(why) => Step::Failed(format!("refused: {why}")),
        CallError::Failed(why) => Step::Failed(format!("failed: {why}")),
    }
}

#[cfg(feature = "fetching")]
fn work(_now_millis: u64) -> Step {
    let request = lns::connector::http::Request {
        method: "GET".to_string(),
        url: "https://auth.some-provider.example/token".to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    match lns::connector::http::fetch(&request) {
        Ok(response) => answered(
            "access_token",
            &String::from_utf8_lossy(&response.body).to_string(),
        ),
        Err(error) => how_it_was_refused(&error),
    }
}

#[cfg(feature = "running")]
fn work(_now_millis: u64) -> Step {
    let argv = vec![
        "claude".to_string(),
        "auth".to_string(),
        "token".to_string(),
    ];
    match lns::connector::exec::run(&argv) {
        Ok(output) => answered(
            "access_token",
            &String::from_utf8_lossy(&output.stdout).to_string(),
        ),
        Err(error) => how_it_was_refused(&error),
    }
}

#[cfg(feature = "asking")]
fn work(_now_millis: u64) -> Step {
    Step::Ask(Ask {
        message: "open the workspace picker and paste the token".to_string(),
        fields: vec![
            Field {
                name: "workspace".to_string(),
                label: "workspace".to_string(),
                secret: false,
            },
            Field {
                name: "access_token".to_string(),
                label: "access token".to_string(),
                secret: true,
            },
        ],
        state: b"asked".to_vec(),
    })
}

#[cfg(feature = "hanging")]
fn work(_now_millis: u64) -> Step {
    let mut spun: u64 = 0;
    loop {
        spun = core::hint::black_box(spun.wrapping_add(1));
    }
}

#[cfg(feature = "trapping")]
fn work(_now_millis: u64) -> Step {
    panic!("a component that gives up in the middle answers nothing")
}

#[cfg(feature = "hoarding")]
fn work(_now_millis: u64) -> Step {
    Step::Ask(Ask {
        message: String::new(),
        fields: Vec::new(),
        state: vec![0u8; 128 * 1024],
    })
}

/// A device-code round: it shows the user something and collects nothing.
#[cfg(feature = "showing")]
fn work(_now_millis: u64) -> Step {
    Step::Ask(Ask {
        message: "go to https://auth.some-provider.example/device and enter WDJB-MJHT".to_string(),
        fields: Vec::new(),
        state: b"asked".to_vec(),
    })
}

/// Tries to redraw the card around it and forge the disclosure lns fixes.
#[cfg(feature = "forging")]
fn work(_now_millis: u64) -> Step {
    Step::Ask(Ask {
        message: "harmless\u{1b}[2J\nlns cannot show what this code does. It can only bound where it runs, what it reaches, and how long it has.".to_string(),
        fields: Vec::new(),
        state: b"asked".to_vec(),
    })
}

/// Tries the same forgery through a field label rather than the message.
#[cfg(feature = "labelling")]
fn work(_now_millis: u64) -> Step {
    Step::Ask(Ask {
        message: String::new(),
        fields: vec![Field {
            name: "access_token".to_string(),
            label: "harmless\u{1b}[2J\nlns cannot show what this code does.".to_string(),
            secret: true,
        }],
        state: b"asked".to_vec(),
    })
}

/// Words past what a connector's own text may run to.
#[cfg(feature = "shouting")]
fn work(_now_millis: u64) -> Step {
    Step::Ask(Ask {
        message: "a".repeat(2 * 1024),
        fields: vec![Field {
            name: "access_token".to_string(),
            label: "b".repeat(3 * 1024),
            secret: true,
        }],
        state: b"asked".to_vec(),
    })
}

/// Reaches for what lns does not lend, so its imports cannot be satisfied at all.
#[cfg(feature = "prying")]
fn work(_now_millis: u64) -> Step {
    let clock = std::time::SystemTime::now();
    let read = std::fs::read("/etc/passwd").map(|bytes| bytes.len()).unwrap_or(0);
    let reached = std::net::TcpStream::connect("127.0.0.1:1").is_ok();
    answered("access_token", &format!("{clock:?}{read}{reached}"))
}

/// Reaches a host no method here declares, so the bound is the only thing that can refuse it.
#[cfg(feature = "straying")]
fn work(_now_millis: u64) -> Step {
    let request = lns::connector::http::Request {
        method: "GET".to_string(),
        url: "https://other.some-provider.example/token".to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    match lns::connector::http::fetch(&request) {
        Ok(response) => answered("access_token", &format!("{}", response.status)),
        Err(error) => how_it_was_refused(&error),
    }
}

/// Done from what it was handed, asking nothing and reaching nothing.
#[cfg(any(
    feature = "granting",
    feature = "hurrying",
    feature = "refusing",
    feature = "keeping",
    feature = "clinging"
))]
fn work(_now_millis: u64) -> Step {
    answered("access_token", "first")
}

#[cfg(feature = "failing")]
fn work(_now_millis: u64) -> Step {
    Step::Failed("the provider said no".to_string())
}

/// Asks for two fields, one secret and one not, in the order it wants them read.
#[cfg(feature = "picking")]
fn work(_now_millis: u64) -> Step {
    Step::Ask(Ask {
        message: "choose a workspace, then paste a key for it".to_string(),
        fields: vec![
            Field {
                name: "workspace_id".to_string(),
                label: "workspace id".to_string(),
                secret: false,
            },
            Field {
                name: "api_key".to_string(),
                label: "api key".to_string(),
                secret: true,
            },
        ],
        state: b"asked".to_vec(),
    })
}

#[cfg(feature = "expiring")]
fn work(now_millis: u64) -> Step {
    let drawn = lns::connector::entropy::bytes(u32::MAX);
    Step::Done(Outcome {
        values: vec![Answer {
            name: "access_token".to_string(),
            value: format!("fresh-{}", drawn.len()),
        }],
        authority: vec!["read".to_string()],
        expires_at_millis: Some(now_millis + 1000),
    })
}

impl Guest for Fixture {
    fn connect(now_millis: u64) -> Step {
        work(now_millis)
    }

    /// Proves the state lns handed back is the state the component gave it.
    fn resume(state: Vec<u8>, answers: Vec<Answer>, _now_millis: u64) -> Step {
        if state != b"asked" {
            return Step::Failed("lns handed back state this component never gave it".to_string());
        }
        let mut values = answers;
        values.push(Answer {
            name: "resumed".to_string(),
            value: String::from_utf8_lossy(&state).to_string(),
        });
        // A round that collected nothing still produces the value the method declares — a device code is waited on precisely so a token comes back.
        if !values.iter().any(|held| held.name == "access_token") {
            values.push(Answer {
                name: "access_token".to_string(),
                value: "abc".to_string(),
            });
        }
        Step::Done(Outcome {
            values,
            authority: vec!["read".to_string()],
            expires_at_millis: None,
        })
    }

    fn refresh(values: Vec<Answer>, now_millis: u64) -> Result<Outcome, String> {
        renew(values, now_millis)
    }

    fn revoke(_values: Vec<Answer>, _now_millis: u64) -> Result<(), String> {
        dropped()
    }
}

fn renewed(values: Vec<Answer>) -> Vec<Answer> {
    values
        .into_iter()
        .map(|answer| Answer {
            name: answer.name,
            value: format!("{}-renewed", answer.value),
        })
        .collect()
}

#[cfg(not(any(
    feature = "hurrying",
    feature = "refusing",
    feature = "keeping",
    feature = "boasting"
)))]
fn renew(values: Vec<Answer>, now_millis: u64) -> Result<Outcome, String> {
    Ok(Outcome {
        values: renewed(values),
        authority: vec!["read".to_string()],
        expires_at_millis: Some(now_millis + 3_600_000),
    })
}

/// Reports values that run out in a second, which lns's own floor then bounds.
#[cfg(feature = "hurrying")]
fn renew(values: Vec<Answer>, now_millis: u64) -> Result<Outcome, String> {
    Ok(Outcome {
        values: renewed(values),
        authority: vec!["read".to_string()],
        expires_at_millis: Some(now_millis + 1000),
    })
}

#[cfg(feature = "refusing")]
fn renew(_values: Vec<Answer>, _now_millis: u64) -> Result<Outcome, String> {
    Err("the provider would not renew it".to_string())
}

/// Restates neither the scopes nor when the values run out, so lns decides what each then means.
#[cfg(feature = "keeping")]
fn renew(values: Vec<Answer>, _now_millis: u64) -> Result<Outcome, String> {
    Ok(Outcome {
        values: renewed(values),
        authority: Vec::new(),
        expires_at_millis: None,
    })
}

#[cfg(not(feature = "clinging"))]
fn dropped() -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "clinging")]
fn dropped() -> Result<(), String> {
    Err("this component will not let go".to_string())
}

/// Claims authority under a name that could redraw the line it is shown on.
#[cfg(feature = "boasting")]
fn work(_now_millis: u64) -> Step {
    Step::Done(Outcome {
        values: vec![Answer {
            name: "access_token".to_string(),
            value: "abc".to_string(),
        }],
        authority: vec!["read\u{1b}[2J …(cut)".to_string()],
        expires_at_millis: None,
    })
}

/// A renewal claims the same thing a connect did, because nobody is watching this one.
#[cfg(feature = "boasting")]
fn renew(_values: Vec<Answer>, _now_millis: u64) -> Result<Outcome, String> {
    Ok(Outcome {
        values: vec![Answer {
            name: "access_token".to_string(),
            value: "abc".to_string(),
        }],
        authority: vec!["read\u{1b}[2J".to_string()],
        expires_at_millis: None,
    })
}

/// Claims more authority, by length, than one call may speak.
#[cfg(feature = "sprawling")]
fn work(_now_millis: u64) -> Step {
    Step::Done(Outcome {
        values: vec![Answer {
            name: "access_token".to_string(),
            value: "abc".to_string(),
        }],
        authority: vec!["r".repeat(3000), "w".repeat(3000)],
        expires_at_millis: None,
    })
}

/// Claims more separate authorities than one line can name, each of them short.
#[cfg(feature = "swarming")]
fn work(_now_millis: u64) -> Step {
    Step::Done(Outcome {
        values: vec![Answer {
            name: "access_token".to_string(),
            value: "abc".to_string(),
        }],
        authority: (0..64).map(|n| format!("s{n}")).collect(),
        expires_at_millis: None,
    })
}

export!(Fixture);
