//! Gate golden tests: the host's fixed rules table over (role, label, app, action), including adversarial labels.
//! A scorer can't change any of these: rules come from the host table only (`extra` can add, never remove).
use qrtz_ctrl::act::Action;
use qrtz_ctrl::ax::{prune, AxNodeWire};
use qrtz_ctrl::gate::rules_for;

fn node(role: &str, title: &str) -> qrtz_ctrl::ax::AxNode {
    let root = AxNodeWire {
        id: "e:1:0".into(),
        role: "AXWindow".into(),
        enabled: true,
        children: vec![AxNodeWire {
            id: "e:1:1".into(),
            role: role.into(),
            title: title.into(),
            actions: vec!["AXPress".into()],
            enabled: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let t = prune("app", None, &root, None);
    t.root.children[0].clone()
}

fn press() -> Action {
    Action::Press {
        handle: "e:1:1".into(),
    }
}

#[test]
fn golden_rules_table() {
    let notes = "com.apple.Notes";
    let mail = "com.apple.mail";
    let settings = "com.apple.systempreferences";
    let cases: &[(&str, &str, &str, &[&str])] = &[
        // role, label, app, expected rules
        ("AXButton", "New Note", notes, &[]),
        ("AXButton", "Send", mail, &["label:send"]),
        ("AXButton", "Ѕend", mail, &["label:send"]), // Cyrillic S
        ("AXButton", "S e n d", mail, &["label:send"]),
        ("AXButton", "s3nd", mail, &["label:send"]),
        ("AXButton", "Se\u{200B}nd", mail, &["label:send"]),
        ("AXButton", "Ｓｅｎｄ", mail, &["label:send"]),
        ("AXButton", "𝐒𝐞𝐧𝐝", mail, &["label:send"]),
        ("AXButton", "Sending…", mail, &["label:send"]),
        ("AXButton", "Buy now", "com.apple.Safari", &["label:buy"]),
        ("AXButton", "Place Order", "com.apple.Safari", &["label:order"]),
        ("AXButton", "Move to Trash", "com.apple.finder", &["label:trash"]),
        ("AXButton", "Log Out", "com.apple.Safari", &["label:logout"]),
        ("AXButton", "Don't Save", notes, &["label:dont"]),
        ("AXMenuItem", "Quit Notes", notes, &["label:quit"]),
        ("AXCheckBox", "Wi-Fi", settings, &["app:system_settings"]),
        (
            "AXButton",
            "Delete",
            settings,
            &["app:system_settings", "label:delete"],
        ),
        ("AXButton", "Sender", mail, &[]),
        ("AXButton", "Sжnd", mail, &["label:mixed_script"]),
    ];
    for (role, label, app, want) in cases {
        let n = node(role, label);
        let got = rules_for(&press(), Some(&n), app, &[], false);
        let want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
        assert_eq!(got, want, "{role} {label:?} in {app}");
    }
}

#[test]
fn app_and_combo_rules() {
    let field = node("AXTextField", "Message");
    let set = |submit| Action::SetValue {
        handle: "e:1:1".into(),
        text_ref: "goal#0..2".into(),
        submit,
    };
    assert_eq!(
        rules_for(&set(true), Some(&field), "com.tinyspeck.slackmacgap", &[], false),
        vec!["app:messaging_submit", "submit"]
    );
    assert!(rules_for(&set(false), Some(&field), "com.tinyspeck.slackmacgap", &[], false).is_empty());
    assert_eq!(
        rules_for(&set(true), Some(&field), "com.apple.Terminal", &[], false),
        vec!["app:terminal_submit", "submit"]
    );
    let key = |c: &str| Action::KeyCombo {
        app: "com.apple.Notes".into(),
        combo: c.into(),
    };
    assert_eq!(
        rules_for(&key("⌘Q"), None, "com.apple.Notes", &[], false),
        vec!["key:quit"]
    );
    assert_eq!(
        rules_for(&key("cmd+delete"), None, "com.apple.Notes", &[], false),
        vec!["key:delete"]
    );
    assert!(rules_for(&key("cmd+n"), None, "com.apple.Notes", &[], false).is_empty());
    // Opening an app or focusing a field never asks, even in System Settings.
    let open = Action::OpenApp {
        app: "com.apple.systempreferences".into(),
    };
    assert!(rules_for(&open, None, "com.apple.systempreferences", &[], false).is_empty());
}

#[test]
fn a_model_can_add_rules_but_never_remove_one() {
    let n = node("AXButton", "Send");
    let got = rules_for(
        &press(),
        Some(&n),
        "com.apple.mail",
        &["model:irreversible".into()],
        false,
    );
    assert_eq!(got, vec!["label:send", "model:irreversible"]);
    // There is no input that removes label:send: the rules come from the label, the app and the combo.
    let got = rules_for(&press(), Some(&n), "com.apple.mail", &[], false);
    assert_eq!(got, vec!["label:send"]);
}

fn w(id: &str, role: &str, title: &str, kids: Vec<AxNodeWire>) -> AxNodeWire {
    AxNodeWire {
        id: id.into(),
        role: role.into(),
        title: title.into(),
        actions: if role == "AXButton" {
            vec!["AXPress".into()]
        } else {
            vec![]
        },
        enabled: true,
        children: kids,
        ..Default::default()
    }
}

/// Review H1: a press inside a sheet or dialog confirms, with that dialog's text as rule input; unlabeled
/// buttons confirm; ancestor labels count; the new words gate.
#[test]
fn dialogs_ancestors_unlabeled_buttons_and_the_new_words() {
    use qrtz_ctrl::gate::context_rules;
    let sheet = w(
        "e:1:2",
        "AXSheet",
        "",
        vec![
            w("e:1:3", "AXStaticText", "Delete 3 items permanently?", vec![]),
            w("e:1:4", "AXButton", "OK", vec![]),
        ],
    );
    let group = w(
        "e:1:5",
        "AXGroup",
        "Archive conversation",
        vec![w("e:1:6", "AXButton", "Go", vec![])],
    );
    let root = w(
        "e:1:0",
        "AXWindow",
        "Mail — reply to bob",
        vec![
            sheet,
            group,
            w("e:1:7", "AXButton", "", vec![]),
            w("e:1:8", "AXButton", "New", vec![]),
        ],
    );
    let t = prune("com.apple.mail", None, &root, None);
    let ok = context_rules(&t.root, "e:1:4");
    assert!(
        ok.contains(&"context:dialog".to_string()) && ok.contains(&"label:delete".to_string()),
        "{ok:?}"
    );
    assert_eq!(context_rules(&t.root, "e:1:6"), vec!["label:archive"]);
    assert!(
        context_rules(&t.root, "e:1:8").is_empty(),
        "the window title isn't inherited"
    );
    let unl = t.root.children.iter().find(|n| n.id == "e:1:7").unwrap();
    assert_eq!(unl.risk, vec!["label:unlabeled"]);
    for l in [
        "Leave",
        "End call",
        "Deactivate",
        "Sell",
        "Withdraw",
        "Book now",
        "Empty Trash",
        "Cancel subscription",
    ] {
        assert!(!qrtz_ctrl::gate::label_risk(l).is_empty(), "{l}");
    }
}

/// Review H2 / M4: every submit confirms unless into a search field or the address bar right after cmd-L; a
/// set-value into a non-empty field confirms.
#[test]
fn submits_confirm_by_default_and_non_empty_fields_confirm() {
    use qrtz_ctrl::gate::submit_exempt;
    let set = |submit| Action::SetValue {
        handle: "e:1:1".into(),
        text_ref: "goal#0..2".into(),
        submit,
    };
    let field = node("AXTextField", "Comment");
    assert_eq!(
        rules_for(&set(true), Some(&field), "com.apple.Notes", &[], false),
        vec!["submit"]
    );
    let ret = Action::KeyCombo {
        app: "com.apple.Notes".into(),
        combo: "return".into(),
    };
    assert_eq!(
        rules_for(&ret, None, "com.apple.Notes", &[], false),
        vec!["submit"]
    );
    let mut search = field.clone();
    search.subrole = Some("AXSearchField".into());
    assert!(submit_exempt(Some(&search), None));
    let mut focused = field.clone();
    focused.focused = true;
    let cmd_l = Action::KeyCombo {
        app: "com.apple.Safari".into(),
        combo: "cmd+l".into(),
    };
    assert!(submit_exempt(Some(&focused), Some(&cmd_l)));
    assert!(
        !submit_exempt(Some(&focused), Some(&ret)),
        "only right after cmd-L"
    );
    assert!(
        !submit_exempt(Some(&field), Some(&cmd_l)),
        "only the focused field"
    );
    let mut full = field.clone();
    full.value_tokens = vec!["x".into()];
    assert_eq!(
        rules_for(&set(false), Some(&full), "com.apple.Notes", &[], false),
        vec!["field:not_empty"]
    );
}
