#![allow(dead_code)]
//! Exercises the Zod generator with awkward types. Writes to $TSROUTE_FIXTURE_DIR
//! (or target/zod-fixture) so the output can be type-checked with `tsc`.

use std::collections::HashMap;

use axum::{extract::Path, Json};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Activity {
    Created { by: String },
    Moved { from: u32, to: u32 },
    Archived,
}

#[derive(Serialize, Deserialize, TS)]
#[serde(untagged)]
pub enum IdOrName {
    Id(u64),
    Name(String),
}

/// A comment thread (recursive).
#[derive(Serialize, Deserialize, TS)]
pub struct Thread {
    pub text: String,
    pub replies: Vec<Thread>,
}

#[derive(Serialize, Deserialize, TS)]
pub struct Stats {
    pub by_label: HashMap<String, usize>,
    pub recent: Vec<(u64, String)>,
    pub flags: [bool; 2],
    pub activity: Vec<Activity>,
    pub who: IdOrName,
    pub ratio: Option<f64>,
    #[serde(rename = "kebab-key")]
    pub kebab: bool,
}

#[derive(Serialize, Deserialize, TS)]
pub struct Wrapper<T, U> {
    pub value: T,
    pub extra: Option<U>,
}

#[tsroute::get("/stats", name = "getStats")]
async fn stats() -> Json<Stats> {
    unimplemented!()
}

#[tsroute::get("/threads/{id}")]
async fn thread(Path(_id): Path<u64>) -> Json<Wrapper<Thread, Vec<Activity>>> {
    unimplemented!()
}

#[tsroute::post("/activity")]
async fn post_activity(Json(_a): Json<Activity>) -> Json<Vec<Activity>> {
    unimplemented!()
}

#[test]
fn zod_fixture() {
    let dir = std::env::var("TSROUTE_FIXTURE_DIR")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../target/zod-fixture").into());
    tsroute::Exporter::new()
        .client(format!("{dir}/client.ts"))
        .react_query(format!("{dir}/hooks.ts"))
        .zod(format!("{dir}/schemas.ts"))
        .export()
        .unwrap();
    // Real serde output, so the schemas can be checked against it at runtime.
    let stats = Stats {
        by_label: HashMap::from([("bug".into(), 3)]),
        recent: vec![(1, "a".into())],
        flags: [true, false],
        activity: vec![
            Activity::Created { by: "ada".into() },
            Activity::Moved { from: 1, to: 2 },
            Activity::Archived,
        ],
        who: IdOrName::Name("ada".into()),
        ratio: None,
        kebab: true,
    };
    let thread = Wrapper {
        value: Thread {
            text: "root".into(),
            replies: vec![Thread {
                text: "child".into(),
                replies: vec![],
            }],
        },
        extra: Some(vec![Activity::Archived]),
    };
    let samples = serde_json::json!({ "getStats": stats, "thread": thread });
    std::fs::write(format!("{dir}/samples.json"), samples.to_string()).unwrap();

    let schemas = std::fs::read_to_string(format!("{dir}/schemas.ts")).unwrap();
    assert!(
        schemas.contains("export const ThreadSchema: z.ZodType<Thread>"),
        "{schemas}"
    );
    assert!(schemas.contains(r#"z.literal("moved")"#), "{schemas}");
    assert!(!schemas.contains("could not be converted"), "{schemas}");
}
