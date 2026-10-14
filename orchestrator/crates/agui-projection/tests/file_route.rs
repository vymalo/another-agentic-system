//! The `href` of a kept file names the route the viewer reads it by (ADR 0032, ADR 0040): the
//! owner's `/api/threads/{id}/artifacts/{sha256}` by default, a share link's own for a reader who
//! has the link and no thread. Nothing else of a frame depends on it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{
    Audience, Connect, FileRoute, Flow, Follow, Frame, History, Projector, Window,
};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, ArtifactData, Event, EventBody, FileRef,
    ThreadState, ThreadStateData, Timestamp, UserId, UserMessageData,
};
use serde_json::{Value, json};
use support::log::{THREAD, meta, thread_id};
use support::pages::limits;

const TOKEN: &str = "Zm9vYmFyYmF6cXV4Zm9vYmFyYmF6cXV4Zm9vYmFyYmE";

fn ev(seq: i64, actor: Actor, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: thread_id(),
        at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
        actor,
        body,
    }
}

fn sha(seq: i64) -> String {
    format!("{seq:064x}")
}

fn file(seq: i64, name: &str, mime: &str) -> Event {
    ev(
        seq,
        Actor::agent(&AgentId::new("plain"), None),
        EventBody::Artifact(ArtifactData {
            name: name.to_owned(),
            mime_type: Some(mime.to_owned()),
            uri: None,
            text: None,
            file: Some(FileRef {
                sha256: sha(seq),
                size: 10,
                filename: Some(name.to_owned()),
            }),
        }),
    )
}

/// A finished thread that handed over a chart and a note.
fn log() -> Vec<Event> {
    vec![
        ev(
            1,
            Actor::user(&UserId::new("alice@example.com")),
            EventBody::UserMessage(UserMessageData::new("make a chart")),
        ),
        ev(
            2,
            Actor::agent(&AgentId::new("plain"), None),
            EventBody::AgentStatus(AgentStatusData {
                status: AgentStatus::Working,
                detail: None,
                tier: None,
            }),
        ),
        file(3, "chart.png", "image/png"),
        file(4, "notes.txt", "text/plain"),
        ev(
            5,
            Actor::system(),
            EventBody::ThreadState(ThreadStateData {
                state: ThreadState::Done,
            }),
        ),
    ]
}

fn hrefs(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e) if e.activity_type == "vymalo.artifact" => {
                serde_json::to_value(&e.content).unwrap()["href"]
                    .as_str()
                    .map(str::to_owned)
            }
            _ => None,
        })
        .collect()
}

/// The frames as JSON, with the resume id beside each event.
fn json_of(frames: &[Frame]) -> Vec<Value> {
    frames
        .iter()
        .map(|f| json!({"id": f.resume_id, "event": serde_json::to_value(&f.event).unwrap()}))
        .collect()
}

fn project(route: FileRoute) -> Vec<Frame> {
    let mut projector = Projector::new(meta()).with_file_route(route);
    log()
        .iter()
        .flat_map(|e| projector.apply(e, Audience::Viewer))
        .collect()
}

fn owners() -> Vec<String> {
    [3, 4]
        .map(|n| format!("/api/threads/{THREAD}/artifacts/{}", sha(n)))
        .to_vec()
}

#[test]
fn the_owners_route_is_the_default() {
    let mut plain = Projector::new(meta());
    let default: Vec<Frame> = log()
        .iter()
        .flat_map(|e| plain.apply(e, Audience::Viewer))
        .collect();
    assert_eq!(hrefs(&default), owners());
    assert_eq!(hrefs(&project(FileRoute::Owner)), owners());
    assert_eq!(FileRoute::default(), FileRoute::Owner);
}

#[test]
fn a_reader_of_a_link_is_given_the_links_own_route() {
    let signed_in = hrefs(&project(FileRoute::shared(TOKEN)));
    assert_eq!(
        signed_in,
        [3, 4]
            .map(|n| format!("/api/shared/{TOKEN}/artifacts/{}", sha(n)))
            .to_vec()
    );
    let public = hrefs(&project(FileRoute::public(TOKEN)));
    assert_eq!(
        public,
        [3, 4]
            .map(|n| format!("/api/public/shared/{TOKEN}/artifacts/{}", sha(n)))
            .to_vec()
    );
    // the thread's id is nowhere in what a reader of a link is given to fetch by
    assert!(signed_in.iter().chain(&public).all(|h| !h.contains(THREAD)));
}

#[test]
fn the_route_changes_the_href_and_nothing_else() {
    let owner = json_of(&project(FileRoute::Owner));
    for route in [FileRoute::shared(TOKEN), FileRoute::public(TOKEN)] {
        let link = project(route.clone());
        // the same frames once the hrefs are put back
        let mut back = json_of(&link);
        let swap = |value: &mut Value| {
            if let Some(content) = value.pointer_mut("/event/content")
                && let Some(href) = content.get("href").and_then(Value::as_str)
            {
                let sha = href.rsplit('/').next().unwrap();
                content["href"] = json!(format!("/api/threads/{THREAD}/artifacts/{sha}"));
            }
        };
        back.iter_mut().for_each(swap);
        assert_eq!(back, owner, "{route:?}");
    }
}

#[test]
fn a_file_that_was_not_kept_has_no_href_by_any_route() {
    let refused = ev(
        3,
        Actor::agent(&AgentId::new("plain"), None),
        EventBody::Artifact(ArtifactData {
            name: "dump".to_owned(),
            mime_type: None,
            uri: None,
            text: None,
            file: None,
        }),
    );
    for route in [
        FileRoute::Owner,
        FileRoute::shared(TOKEN),
        FileRoute::public(TOKEN),
    ] {
        let mut projector = Projector::new(meta()).with_file_route(route);
        let frames: Vec<Frame> = projector.apply(&refused, Audience::Viewer);
        assert!(hrefs(&frames).is_empty());
    }
}

#[test]
fn a_connect_stream_names_the_route_it_was_given() {
    let events = log();
    let head = events.last().unwrap().seq;
    let mut connect =
        Connect::new(meta(), 0, head, Follow::Forever).with_file_route(FileRoute::public(TOKEN));
    let frames: Vec<Frame> = events.iter().flat_map(|e| connect.feed(e)).collect();
    assert_eq!(
        hrefs(&frames),
        [3, 4]
            .map(|n| format!("/api/public/shared/{TOKEN}/artifacts/{}", sha(n)))
            .to_vec()
    );
}

#[test]
fn a_history_page_and_its_carry_name_the_route_it_was_given() {
    let events = log();
    let head = events.last().unwrap().seq;
    // a second turn after the files, so the first page carries the files of the turn before it
    let mut more = events.clone();
    more.push(ev(
        6,
        Actor::user(&UserId::new("alice@example.com")),
        EventBody::UserMessage(UserMessageData::new("and again")),
    ));
    more.push(file(7, "again.png", "image/png"));
    more.push(ev(
        8,
        Actor::system(),
        EventBody::ThreadState(ThreadStateData {
            state: ThreadState::Done,
        }),
    ));
    let _ = head;
    let head = more.last().unwrap().seq;
    let route = FileRoute::shared(TOKEN);
    let mut history = History::new(
        meta(),
        Window::Turns {
            before: None,
            turns: 1,
        },
        limits(100, usize::MAX),
        head,
    )
    .with_file_route(route);
    for event in &more {
        if history.feed(event) == Flow::Done {
            break;
        }
    }
    let page = history.finish();
    assert_eq!(
        hrefs(&page.frames),
        vec![format!("/api/shared/{TOKEN}/artifacts/{}", sha(7))]
    );
    let carry = page
        .carry
        .expect("a page with older turns carries")
        .to_value();
    let carried: Vec<&str> = carry["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["href"].as_str().unwrap())
        .collect();
    assert_eq!(
        carried,
        [3, 4]
            .map(|n| format!("/api/shared/{TOKEN}/artifacts/{}", sha(n)))
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
}
