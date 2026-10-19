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
        let got = rules_for(&press(), Some(&n), app, &[]);
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
        rules_for(&set(true), Some(&field), "com.tinyspeck.slackmacgap", &[]),
        vec!["app:messaging_submit"]
    );
    assert!(rules_for(&set(false), Some(&field), "com.tinyspeck.slackmacgap", &[]).is_empty());
    assert_eq!(
        rules_for(&set(true), Some(&field), "com.apple.Terminal", &[]),
        vec!["app:terminal_submit"]
    );
    let key = |c: &str| Action::KeyCombo {
        app: "com.apple.Notes".into(),
        combo: c.into(),
    };
    assert_eq!(
        rules_for(&key("⌘Q"), None, "com.apple.Notes", &[]),
        vec!["key:quit"]
    );
    assert_eq!(
        rules_for(&key("cmd+delete"), None, "com.apple.Notes", &[]),
        vec!["key:delete"]
    );
    assert!(rules_for(&key("cmd+n"), None, "com.apple.Notes", &[]).is_empty());
    // Opening an app or focusing a field never asks, even in System Settings.
    let open = Action::OpenApp {
        app: "com.apple.systempreferences".into(),
    };
    assert!(rules_for(&open, None, "com.apple.systempreferences", &[]).is_empty());
}

#[test]
fn a_model_can_add_rules_but_never_remove_one() {
    let n = node("AXButton", "Send");
    let got = rules_for(
        &press(),
        Some(&n),
        "com.apple.mail",
        &["model:irreversible".into()],
    );
    assert_eq!(got, vec!["label:send", "model:irreversible"]);
    // There is no input that removes label:send: the rules come from the label, the app and the combo.
    let got = rules_for(&press(), Some(&n), "com.apple.mail", &[]);
    assert_eq!(got, vec!["label:send"]);
}
