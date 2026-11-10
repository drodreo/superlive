//! Asset protocol scope tests.
//!
//! The shell enables tauri's asset protocol (`asset://`) so component UIs
//! (webview iframes) can read local media files. The config ships a deny
//! list; these tests anchor its semantics against the real scope engine
//! (`tauri::scope::fs::Scope` — the exact code path the asset protocol
//! handler consults), not against a re-implementation.
//!
//! Platform context (tauri 2.12 `scope/fs.rs::Scope::new`): without an
//! explicit `requireLiteralLeadingDot`, the scope requires a literal dot for
//! wildcard components on unix and does not on windows. The shell test suite
//! is excluded from Windows CI, so the windows behavior is anchored here by
//! constructing the scope with `Some(false)` explicitly — that variant
//! compiles and runs on linux and pins the semantics the windows default
//! resolves to.

use std::fs;
use std::path::PathBuf;

use tauri::Manager;
use tauri::scope::fs::Scope;
use tauri::utils::config::FsScope;

/// Parses the crate's own tauri.conf.json and asserts the asset protocol
/// block: enabled, allow-all, the three deny entries, and no explicit
/// dot-semantics override (platform default applies at runtime).
#[test]
fn config_declares_deny_scope_with_platform_default_dot_semantics() {
    let raw = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"));
    let config: serde_json::Value = serde_json::from_str(raw).unwrap();
    let asset = &config["app"]["security"]["assetProtocol"];
    assert_eq!(asset["enable"], serde_json::json!(true));

    let scope: FsScope = serde_json::from_value(asset["scope"].clone()).unwrap();
    let FsScope::Scope {
        allow,
        deny,
        require_literal_leading_dot,
    } = scope
    else {
        panic!(
            "assetProtocol.scope must be the full scope form (allow/deny), not a bare path list"
        );
    };
    assert_eq!(allow, vec![PathBuf::from("**")]);
    assert_eq!(
        deny,
        vec![
            PathBuf::from("$HOME/.ssh/**"),
            PathBuf::from("$HOME/.gnupg/**"),
            PathBuf::from("$APPDATA/**"),
        ]
    );
    // Absent on purpose: tauri resolves the platform default (true on unix,
    // false on windows). Both behaviors are pinned by the explicit-variant
    // tests below.
    assert_eq!(require_literal_leading_dot, None);
}

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    /// A regular media file outside any hidden directory.
    media: PathBuf,
    /// SSH private key inside a hidden directory covered by deny.
    ssh_key: PathBuf,
    /// GPG secret key, nested deeper inside a hidden directory covered by deny.
    gpg_key: PathBuf,
    /// Hidden directory NOT covered by deny — isolates the wildcard/dot-dir
    /// semantics from the deny list.
    hidden_unrelated: PathBuf,
    /// Stand-in for the shell data root (`$APPDATA` resolves to
    /// `app_data_dir()`, the shell's own data directory) covered by deny.
    appdata_file: PathBuf,
}

/// Creates real files for every probe path: the scope engine canonicalizes
/// paths before matching, and a nonexistent path always fails closed —
/// probing phantom paths would make the assertions vacuous.
fn sandbox() -> Sandbox {
    let dir = tempfile::Builder::new()
        .prefix("sl-asset-scope-")
        .tempdir()
        .unwrap();
    let root = dir.path().to_path_buf();
    let media = root.join("media/clip.mp4");
    let ssh_key = root.join(".ssh/id_ed25519");
    let gpg_key = root.join(".gnupg/private-keys-v1.d/secret.pgp");
    let hidden_unrelated = root.join(".cache/notes.txt");
    let appdata_file = root.join("appdata/shell-data/component.log");
    for path in [&media, &ssh_key, &gpg_key, &hidden_unrelated, &appdata_file] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"probe").unwrap();
    }
    Sandbox {
        _dir: dir,
        root,
        media,
        ssh_key,
        gpg_key,
        hidden_unrelated,
        appdata_file,
    }
}

/// Builds the production deny-list shape against the sandbox root.
fn sandbox_scope(sb: &Sandbox, dot_semantics: Option<bool>) -> FsScope {
    FsScope::Scope {
        allow: vec![sb.root.join("**")],
        deny: vec![
            sb.root.join(".ssh/**"),
            sb.root.join(".gnupg/**"),
            sb.root.join("appdata/**"),
        ],
        require_literal_leading_dot: dot_semantics,
    }
}

/// The platform default (no explicit `requireLiteralLeadingDot`): sensitive
/// paths are denied by the deny list on every platform; hidden directories
/// additionally fail closed on unix (wildcards never enter dot dirs there).
#[test]
fn default_dot_semantics_deny_sensitive_dirs_and_fail_closed_on_unix() {
    let app = tauri::test::mock_app();
    let sb = sandbox();
    let scope = Scope::new(&app, &sandbox_scope(&sb, None)).unwrap();

    assert!(scope.is_allowed(&sb.media));

    // Deny list wins over allow on every platform (deny is checked first and
    // is absolute), and the match is reported as an explicit forbid.
    assert!(!scope.is_allowed(&sb.ssh_key));
    assert!(scope.is_forbidden(&sb.ssh_key));
    assert!(!scope.is_allowed(&sb.gpg_key));
    assert!(scope.is_forbidden(&sb.gpg_key));
    assert!(!scope.is_allowed(&sb.appdata_file));
    assert!(scope.is_forbidden(&sb.appdata_file));

    // Platform default for the un-denied hidden file: on unix wildcards
    // cannot match dot components, so the file is neither forbidden nor
    // allowed (default deny). On windows wildcards do match dot components,
    // so the file is allowed — the explicit windows anchor lives in the
    // sibling test below.
    #[cfg(unix)]
    {
        assert!(!scope.is_allowed(&sb.hidden_unrelated));
        assert!(!scope.is_forbidden(&sb.hidden_unrelated));
    }
    #[cfg(windows)]
    assert!(scope.is_allowed(&sb.hidden_unrelated));
}

/// Windows semantics anchored explicitly (`requireLiteralLeadingDot: false`,
/// which is what the windows platform default resolves to): wildcards match
/// dot directories, so the allow-all list would reach every sensitive path —
/// the deny list is the only line of defense. Runs on linux because the
/// shell test suite is excluded from Windows CI.
#[test]
fn windows_dot_semantics_rely_on_the_deny_list() {
    let app = tauri::test::mock_app();
    let sb = sandbox();
    let scope = Scope::new(&app, &sandbox_scope(&sb, Some(false))).unwrap();

    assert!(scope.is_allowed(&sb.media));

    // With wildcards allowed into dot dirs, deny is what keeps the
    // sensitive paths out — each one must be an explicit forbid.
    assert!(!scope.is_allowed(&sb.ssh_key));
    assert!(scope.is_forbidden(&sb.ssh_key));
    assert!(!scope.is_allowed(&sb.gpg_key));
    assert!(scope.is_forbidden(&sb.gpg_key));
    assert!(!scope.is_allowed(&sb.appdata_file));
    assert!(scope.is_forbidden(&sb.appdata_file));

    // Differential anchor against the unix behavior: the same un-denied
    // hidden file flips to allowed once wildcards may match dot components.
    assert!(scope.is_allowed(&sb.hidden_unrelated));
}

/// Unix semantics anchored explicitly (`requireLiteralLeadingDot: true`,
/// which is what the unix platform default resolves to): wildcards never
/// enter dot directories, so even paths the deny list does not mention fail
/// closed. Runs on every platform.
#[test]
fn unix_dot_semantics_fail_closed_for_hidden_dirs() {
    let app = tauri::test::mock_app();
    let sb = sandbox();
    let scope = Scope::new(&app, &sandbox_scope(&sb, Some(true))).unwrap();

    assert!(scope.is_allowed(&sb.media));
    assert!(!scope.is_allowed(&sb.ssh_key));
    assert!(scope.is_forbidden(&sb.ssh_key));
    // The deny list does not mention .cache; unix dot semantics alone keep
    // it out of the allow-all wildcard.
    assert!(!scope.is_allowed(&sb.hidden_unrelated));
    assert!(!scope.is_forbidden(&sb.hidden_unrelated));
}

/// The deny entry `$APPDATA/**` must resolve to the same directory the shell
/// uses as its data root (`app_data_dir()` — components, logs and the
/// registry all live under it), so component webviews cannot read the
/// shell's own data tree through asset:// URLs.
#[test]
fn appdata_variable_resolves_to_the_shell_data_root() {
    let app = tauri::test::mock_app();
    assert_eq!(
        app.path().parse("$APPDATA").unwrap(),
        app.path().app_data_dir().unwrap()
    );
    // The other two deny entries rely on the same variable substitution.
    assert_eq!(
        app.path().parse("$HOME").unwrap(),
        app.path().home_dir().unwrap()
    );
}
