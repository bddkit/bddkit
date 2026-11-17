use super::{init_with, pack_files, spawn, temp, text};
use crate::common;
use std::process::Command;

/// `init --db` writes `resources.db.main` and installs the plugin the index
/// names for `db`, with no question about plugins: the databases are a plugin
/// group. The group fixture, claiming `db` alone, stands in for `bddkit-db`,
/// which the index on this branch does not list yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_with_an_api_url_installs_the_api_plugin() {
    let (stub, github) = spawn().await;
    stub.index("plugin:\n  - name: api\n    repo: bddkit/bddkit-api\n    description: HTTP API\n    groups: [api]\n");
    stub.publish_archive("bddkit/bddkit-api", "0.1.0", |top| {
        let lib = std::fs::read(common::build_group_plugin(false)).expect("fixture");
        pack_files(top, &[(&common::dll_name("group_plugin"), lib.as_slice())])
    });
    stub.latest("bddkit/bddkit-api", "0.1.0");
    let dir = temp("init-api");
    let out = init_with(
        &dir,
        &github,
        &[
            "--api-url",
            "http://127.0.0.1:8080",
            "--parallel",
            "false",
            "--telemetry",
            "false",
        ],
        &[("BDDKIT_GROUP_FIXTURE_HOST_GROUPS", "1")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let config = std::fs::read_to_string(dir.join("bddkit.yaml")).expect("config written");
    assert!(
        config.contains("base_url: http://127.0.0.1:8080"),
        "{config}"
    );
    let lock = std::fs::read_to_string(dir.join("xdg-config/bddkit/plugins.yaml"))
        .expect("the plugin went to the user layer");
    assert!(lock.contains("source: bddkit/bddkit-api"), "{lock}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_with_a_database_installs_the_db_plugin() {
    let (stub, github) = spawn().await;
    stub.index("plugin:\n  - name: db\n    repo: bddkit/bddkit-db\n    description: Databases\n    groups: [db]\n");
    stub.publish_archive("bddkit/bddkit-db", "0.1.0", |top| {
        let lib = std::fs::read(common::build_group_plugin(false)).expect("fixture");
        pack_files(top, &[(&common::dll_name("group_plugin"), lib.as_slice())])
    });
    stub.latest("bddkit/bddkit-db", "0.1.0");
    let dir = temp("init-db");
    let fixture = ("BDDKIT_GROUP_FIXTURE_HOST_GROUPS", "db");
    let log = dir.join("t.log");
    let out = init_with(
        &dir,
        &github,
        &[
            "--no-api",
            "--db",
            "postgres",
            "--dsn",
            // `$` survives the round trip: `$$` stays two, a reference stays one.
            "postgres://u:pa$$w@h/db?x=${BDDKIT_INIT_TEST_UNSET:-d}",
            "--parallel",
            "false",
            "--telemetry",
            "true",
        ],
        &[
            fixture,
            ("BDDKIT_TELEMETRY_LOG", log.to_str().expect("utf-8")),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let config = std::fs::read_to_string(dir.join("bddkit.yaml")).expect("config written");
    assert!(config.contains("db:\n    main:\n      dsn: "), "{config}");
    assert!(config.contains("pa$$$$w"), "{config}");
    assert!(config.contains("${BDDKIT_INIT_TEST_UNSET:-d}"), "{config}");
    let lock = std::fs::read_to_string(dir.join("xdg-config/bddkit/plugins.yaml"))
        .expect("the plugin went to the user layer");
    assert!(lock.contains("source: bddkit/bddkit-db"), "{lock}");

    // The instance is counted under the plugin's own name, as an official one:
    // a database engine is no longer the host's to know.
    let payloads = common::read_payloads(&log);
    assert_eq!(payloads.len(), 1, "{payloads:?}");
    let resources = &payloads[0]["init"]["resources"];
    assert_eq!(resources["plugins"]["group"], 1, "{resources}");
    assert!(resources.get("db").is_none(), "{resources}");
    let raw = std::fs::read_to_string(&log).expect("payloads");
    assert!(
        !raw.contains("pa$"),
        "the DSN never reaches the payload: {raw}"
    );

    let doctor = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .env("HOME", &*dir)
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env_remove("BDDKIT_CONFIG")
        .env_remove("BDDKIT_DIR")
        .env(fixture.0, fixture.1)
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(doctor.status.code(), Some(0), "{}", text(&doctor));

    // The recommended DSN, a reference not exported yet: the config does not
    // load until it is, and the plugin is installed all the same.
    let unset = temp("init-db-unset");
    let out = init_with(
        &unset,
        &github,
        &[
            "--no-api",
            "--db",
            "postgres",
            "--dsn",
            "${BDDKIT_INIT_TEST_UNSET_DSN}",
            "--parallel",
            "false",
        ],
        &[fixture],
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let lock = std::fs::read_to_string(unset.join("xdg-config/bddkit/plugins.yaml"))
        .unwrap_or_else(|_| panic!("the plugin was installed:\n{}", text(&out)));
    assert!(lock.contains("source: bddkit/bddkit-db"), "{lock}");
}
