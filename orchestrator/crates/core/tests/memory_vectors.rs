#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The memory-v1 vectors (`docs/api/examples/memory/*.json`), run against the pure core.
//!
//! The web's mock server runs the same files (`web/mock/memory.test.ts`) with the same conventions:
//! `holdNotes: n` holds the notes `I deploy with Helm`, `I use pnpm`, `staging is kestrel`, then
//! `filler 4`, `filler 5`, ...; person A's ids are `m-a-<i>` and B's `m-b-<i>`, from 1.

use std::collections::BTreeMap;
use std::path::PathBuf;

use orch_core::{
    ForgetInput, Grant, Memory, MemoryError, MemorySettings, MemoryTool, ReadInput, Refusal,
    SaveInput, ThreadSwitch, check_call, listed,
};
use serde_json::Value;

fn vectors(file: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/api/examples/memory")
        .join(file);
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("vector file")).expect("json");
    doc["cases"].as_array().expect("cases").clone()
}

const SEED: [&str; 3] = ["I deploy with Helm", "I use pnpm", "staging is kestrel"];

fn hold(m: &mut Memory, who: &str, n: usize) {
    for i in 1..=n {
        let text = SEED
            .get(i - 1)
            .map_or_else(|| format!("filler {i}"), |s| (*s).to_owned());
        let text = SaveInput::parse(&serde_json::json!({ "text": text })).unwrap();
        m.save(
            text,
            "person",
            format!("m-{who}-{i}"),
            format!("2026-11-24T09:{i:02}:00Z"),
        )
        .unwrap();
    }
}

fn texts(m: &mut Memory, who: &str, texts: &[Value]) {
    for (i, t) in texts.iter().enumerate() {
        let text = SaveInput::parse(&serde_json::json!({ "text": t })).unwrap();
        m.save(
            text,
            "person",
            format!("m-{who}-{}", i + 1),
            format!("2026-11-24T09:{:02}:00Z", i + 1),
        )
        .unwrap();
    }
}

fn input_of(v: &Value) -> Value {
    // `{"$repeat": "x", "times": n}` stands for a long text.
    match v {
        Value::Object(o) if o.contains_key("$repeat") => {
            let unit = o["$repeat"].as_str().unwrap();
            Value::String(unit.repeat(o["times"].as_u64().unwrap() as usize))
        }
        Value::Object(o) => {
            Value::Object(o.iter().map(|(k, x)| (k.clone(), input_of(x))).collect())
        }
        other => other.clone(),
    }
}

fn tool_of(call: &Value) -> MemoryTool {
    MemoryTool::from_name(call["tool"].as_str().unwrap()).expect("a memory tool")
}

/// Runs a call and returns `Ok(result summary)` or the refusal code.
fn run_call(
    m: &mut Memory,
    call: &Value,
    switch: ThreadSwitch,
    grant: Option<Grant>,
) -> Result<Value, String> {
    let tool = tool_of(call);
    let main = call["caller"] == "main";
    match check_call(tool, switch, main, grant) {
        Err(Refusal::UnknownTool) => return Err("unknown tool".into()),
        Err(Refusal::Refused(e)) => return Err(e.code().into()),
        Ok(()) => {}
    }
    let input = input_of(&call["input"]);
    let agent = call["agent"].as_str().unwrap();
    let r = match tool {
        MemoryTool::Read => ReadInput::parse(&input).map(|i| {
            let r = m.read(&i);
            serde_json::json!({
                "texts": r.notes.iter().map(|n| n.text.clone()).collect::<Vec<_>>(),
                "count": r.notes.len(), "total": r.total, "truncated": r.truncated,
                "newestFirst": r.notes.windows(2).all(|w| w[0].saved_at >= w[1].saved_at),
            })
        }),
        MemoryTool::Save => SaveInput::parse(&input).and_then(|t| {
            let n = m.len() + 1;
            m.save(
                t,
                agent,
                format!("m-new-{n}"),
                "2026-11-24T10:00:00Z".into(),
            )
            .map(|n| serde_json::json!({ "id": n.id, "savedAt": n.saved_at }))
        }),
        MemoryTool::Forget => {
            ForgetInput::parse(&input).map(|i| serde_json::json!({ "forgotten": m.forget(&i) }))
        }
    };
    r.map_err(|e: MemoryError| e.code().into())
}

#[test]
fn tools_vectors() {
    let cases = vectors("tools.json");
    assert!(cases.len() >= 10, "vector count");
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let switch = if case["switch"] == "on" {
            ThreadSwitch::On
        } else {
            ThreadSwitch::Off
        };
        let grants: BTreeMap<String, Grant> = case["grants"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    Grant {
                        read: v["read"] == true,
                        write: v["write"] == true,
                    },
                )
            })
            .collect();
        let call = &case["call"];
        let grant = grants.get(call["agent"].as_str().unwrap()).copied();
        let mut m = Memory::default();
        if let Some(n) = case["holdNotes"].as_u64() {
            hold(&mut m, "a", n as usize);
        }
        let expect = &case["expect"];
        let out = run_call(&mut m, call, switch, grant);
        if let Some(code) = expect["error"].as_str() {
            assert_eq!(out.as_ref().err().map(String::as_str), Some(code), "{name}");
        } else {
            let result = out.unwrap_or_else(|e| panic!("{name}: refused {e}"));
            if let Some(r) = expect["result"].as_object() {
                for (k, v) in r {
                    let got = &result[k];
                    // "…" in a vector stands for a generated value.
                    if v.as_str().is_some_and(|s| s.ends_with('…')) {
                        assert!(
                            got.as_str().is_some_and(
                                |g| g.starts_with(v.as_str().unwrap().trim_end_matches('…'))
                            ),
                            "{name}: {k}"
                        );
                    } else {
                        assert_eq!(got, v, "{name}: {k}");
                    }
                }
            }
            if let Some(n) = expect["resultNotes"].as_u64() {
                assert_eq!(result["count"], n, "{name}: resultNotes");
            }
            for k in ["total", "truncated"] {
                if !expect[k].is_null() {
                    assert_eq!(result[k], expect[k], "{name}: {k}");
                }
            }
            if expect["order"] == "newest first" {
                assert_eq!(result["newestFirst"], true, "{name}: order");
            }
            if expect["storedImmediately"] == true {
                assert_eq!(m.len(), 1, "{name}: stored at once");
            }
        }
        if let Some(n) = expect["notesAfter"].as_u64() {
            assert_eq!(m.len() as u64, n, "{name}: notesAfter");
        }
        if let Some(b) = expect["namesBound"].as_u64() {
            assert_eq!(b as usize, orch_core::MAX_NOTE_BYTES, "{name}");
            assert!(
                !MemoryError::TooLong.to_string().contains("xxx"),
                "{name}: never echoes the text"
            );
        }
        if let Some(l) = expect["listed"].as_bool() {
            let tool = tool_of(call);
            assert_eq!(
                listed(tool, switch, call["caller"] == "main", grant),
                l,
                "{name}: listed"
            );
        }
    }
}

#[test]
fn isolation_vectors() {
    let cases = vectors("isolation.json");
    assert!(cases.len() >= 4, "vector count");
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        // Every person has a memory of their own: the isolation is that no call reaches another's.
        let mut people: BTreeMap<&str, Memory> = BTreeMap::new();
        for who in ["a", "b"] {
            let mut m = Memory::default();
            texts(&mut m, who, case["setup"][who]["notes"].as_array().unwrap());
            people.insert(who, m);
        }
        let call = &case["call"];
        let who = call["as"].as_str().unwrap();
        let grant = Some(Grant {
            read: true,
            write: true,
        });
        let out = run_call(people.get_mut(who).unwrap(), call, ThreadSwitch::On, grant).unwrap();
        let e = &case["expect"];
        if let Some(n) = e["resultNotes"].as_u64() {
            assert_eq!(out["count"], n, "{name}");
        }
        if let Some(t) = e["texts"].as_array() {
            assert_eq!(&out["texts"], &Value::Array(t.clone()), "{name}");
        }
        for k in ["total", "truncated"] {
            if !e[k].is_null() {
                assert_eq!(out[k], e[k], "{name}: {k}");
            }
        }
        if let Some(r) = e["result"].as_object() {
            for (k, v) in r {
                assert_eq!(&out[k], v, "{name}: {k}");
            }
        }
        for (key, who) in [("aNotesAfter", "a"), ("bNotesAfter", "b")] {
            if let Some(n) = e[key].as_u64() {
                assert_eq!(people[who].len() as u64, n, "{name}: {key}");
            }
        }
    }
}

#[test]
fn settings_vectors() {
    let cases = vectors("settings.json");
    assert!(cases.len() >= 6, "vector count");
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        // Each case starts from a person who never touched memory.
        let mut s = MemorySettings::default();
        let call = &case["call"];
        let e = &case["expect"];
        let mut events: Vec<&str> = vec![];
        let mut status = 200;
        if call["method"] == "PUT" {
            match s.apply(&call["body"]) {
                Ok(_) => events.push("memory_switched"),
                Err(_) => status = 400,
            }
        }
        assert_eq!(
            status as u64,
            e["status"].as_u64().unwrap(),
            "{name}: status"
        );
        if let Some(ev) = e["events"].as_array() {
            let want: Vec<&str> = ev.iter().map(|x| x.as_str().unwrap()).collect();
            assert_eq!(events, want, "{name}: events");
        }
        let wire = |g: Grant| serde_json::json!({ "read": g.read, "write": g.write });
        let agents: Value = s
            .agents
            .iter()
            .map(|(k, g)| (k.clone(), wire(*g)))
            .collect::<serde_json::Map<_, _>>()
            .into();
        let body = serde_json::json!({ "default": wire(s.default), "agents": agents });
        if e["body"].is_object() {
            assert_eq!(body, e["body"], "{name}: body");
        }
        if e["agents"].is_object() {
            // Where a vector names one agent, the other agents it does not name are unchecked.
            for (k, v) in e["agents"].as_object().unwrap() {
                assert_eq!(&body["agents"][k], v, "{name}: agent {k}");
            }
        }
        if e["code"] == "invalid" {
            assert_eq!(status, 400, "{name}");
        }
    }
}
