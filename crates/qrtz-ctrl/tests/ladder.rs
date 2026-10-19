//! The never-takes-over-the-ui ladder (ADR 0100): which route wins, and which are refused outright.
use qrtz_ctrl::ladder::*;

const SAFARI: &str = "com.apple.Safari";
const EDITOR: &str = "com.microsoft.VSCode";

fn granted() -> Env {
    Env {
        frontmost_app: Some(EDITOR.into()),
        user_idle_ms: Some(10),
        secure_input: false,
        files: Some(Consent::Granted),
        accessibility: Some(Consent::Granted),
        post_event: Some(Consent::Granted),
        automation: [(SAFARI.to_string(), Consent::Granted)].into(),
        quarantined: Default::default(),
        running_apps: [SAFARI.to_string(), EDITOR.to_string()].into(),
    }
}

fn r(id: &str) -> ContentRef {
    ContentRef(id.into())
}

fn ax(app: &str, role: &str, action: &str) -> Route {
    Route::Ax {
        app: app.into(),
        role: role.into(),
        element: r("el"),
        op: AxOp::Perform {
            action: action.into(),
        },
    }
}

fn ax_set(app: &str, role: &str, attribute: &str) -> Route {
    Route::Ax {
        app: app.into(),
        role: role.into(),
        element: r("el"),
        op: AxOp::SetAttribute {
            attribute: attribute.into(),
        },
    }
}

fn key(app: &str, combo: &str) -> Route {
    Route::PidKey {
        app: app.into(),
        combo: combo.into(),
    }
}

fn refused(v: &Verdict) -> Option<&Refusal> {
    match v {
        Verdict::Refused { why } => Some(why),
        _ => None,
    }
}

#[test]
fn the_lowest_allowed_rung_wins_regardless_of_planner_order() {
    // "close all my safari tabs": the planner offers ⌘W keys, an AX close button, and the scripting verb.
    let routes = vec![
        key(SAFARI, "cmd+w"),
        ax(SAFARI, "AXButton", "AXPress"),
        Route::Scripting {
            app: SAFARI.into(),
            verb: ScriptVerb::CloseAllTabs,
        },
    ];
    let (d, v) = choose(&routes, &granted());
    assert_eq!(
        d,
        Decision::Execute {
            index: 2,
            rung: Rung::Scripting
        }
    );
    assert!(v.iter().all(|v| *v == Verdict::Allowed));
}

#[test]
fn files_beat_everything() {
    let routes = vec![
        ax("com.apple.finder", "AXRow", "AXPress"),
        Route::Files(FsOp::Trash { src: r("f1") }),
    ];
    assert_eq!(
        choose(&routes, &granted()).0,
        Decision::Execute {
            index: 1,
            rung: Rung::Files
        }
    );
}

#[test]
fn takeover_routes_are_always_refused() {
    let env = granted();
    for route in [
        Route::Click { app: SAFARI.into() },
        Route::GlobalKey {
            combo: "cmd+w".into(),
        },
        Route::Activate { app: SAFARI.into() },
    ] {
        assert_eq!(route.rung(), Rung::TakesOver);
        assert_eq!(
            judge(&route, &env),
            Verdict::Refused {
                why: Refusal::TakesOverUi
            },
            "{route:?}"
        );
    }
    let (d, _) = choose(
        &[
            Route::Click { app: SAFARI.into() },
            Route::Activate { app: SAFARI.into() },
        ],
        &env,
    );
    assert_eq!(d, Decision::Refuse);
}

#[test]
fn ax_actions_that_raise_open_menus_or_move_focus_are_refused() {
    let env = granted();
    for a in ["AXRaise", "AXShowMenu", "AXPick"] {
        assert!(matches!(
            refused(&judge(&ax(SAFARI, "AXWindow", a), &env)),
            Some(Refusal::ForbiddenAxAction { .. })
        ));
    }
    for role in [
        "AXMenuItem",
        "AXMenuBarItem",
        "AXPopUpButton",
        "AXMenuButton",
        "AXComboBox",
    ] {
        assert_eq!(
            refused(&judge(&ax(SAFARI, role, "AXPress"), &env)),
            Some(&Refusal::OpensMenu { role: role.into() }),
            "{role}"
        );
    }
    for attr in [
        "AXFocused",
        "AXMain",
        "AXFrontmost",
        "AXPosition",
        "AXSize",
        "AXFullScreen",
        // ADR 0187: held until the Mac probe; both ways.
        "AXMinimized",
    ] {
        assert!(matches!(
            refused(&judge(&ax_set(SAFARI, "AXWindow", attr), &env)),
            Some(Refusal::ForbiddenAxAttribute { .. })
        ));
    }
    // What stays allowed in a background app: pressing in-window buttons, writing values, increment, cancel.
    for route in [
        ax(SAFARI, "AXButton", "AXPress"),
        ax(SAFARI, "AXSlider", "AXIncrement"),
        ax(SAFARI, "AXSheet", "AXCancel"),
        ax_set(SAFARI, "AXTextField", "AXValue"),
        ax_set(SAFARI, "AXCheckBox", "AXValue"),
    ] {
        assert_eq!(judge(&route, &env), Verdict::Allowed, "{route:?}");
    }
}

#[test]
fn secure_fields_are_never_touched() {
    let env = granted();
    assert_eq!(
        refused(&judge(&ax_set(SAFARI, "AXSecureTextField", "AXValue"), &env)),
        Some(&Refusal::SecureField)
    );
    assert_eq!(
        refused(&judge(&ax(SAFARI, "AXSecureTextField", "AXPress"), &env)),
        Some(&Refusal::SecureField)
    );
}

#[test]
fn focus_intent_shortcuts_are_refused_even_to_a_background_pid() {
    let env = granted();
    for c in [
        "cmd+l",
        "⌘L",
        "shift+cmd+g",
        "cmd+1",
        "cmd+shift+]",
        "ctrl+tab",
        "cmd+tab",
    ] {
        assert!(
            matches!(
                refused(&judge(&key(SAFARI, c), &env)),
                Some(Refusal::FocusIntentShortcut { .. })
            ),
            "{c}"
        );
    }
    assert_eq!(judge(&key(SAFARI, "cmd+w"), &env), Verdict::Allowed);
}

#[test]
fn keys_never_go_to_the_app_the_user_is_in() {
    let mut env = granted();
    env.frontmost_app = Some(SAFARI.into());
    env.user_idle_ms = Some(60_000); // even when they're idle: they may start typing mid-sequence
    assert_eq!(
        refused(&judge(&key(SAFARI, "cmd+w"), &env)),
        Some(&Refusal::TargetIsFrontmost)
    );
}

#[test]
fn secure_input_defers_key_events() {
    let mut env = granted();
    env.secure_input = true;
    assert_eq!(
        judge(&key(SAFARI, "cmd+w"), &env),
        Verdict::Defer {
            why: DeferReason::SecureInput
        }
    );
    assert_eq!(
        choose(&[key(SAFARI, "cmd+w")], &env).0,
        Decision::Defer {
            retry_after_ms: DEFER_MS
        }
    );
}

#[test]
fn an_active_user_in_the_target_app_defers_ax_and_scripting() {
    let mut env = granted();
    env.frontmost_app = Some(SAFARI.into());
    env.user_idle_ms = Some(300);
    let defer = Verdict::Defer {
        why: DeferReason::UserActiveInTarget,
    };
    assert_eq!(judge(&ax(SAFARI, "AXButton", "AXPress"), &env), defer);
    assert_eq!(
        judge(
            &Route::Scripting {
                app: SAFARI.into(),
                verb: ScriptVerb::CloseAllTabs
            },
            &env
        ),
        defer
    );
    // Once they've been idle for a while, background-safe rungs may run in the app they're looking at.
    env.user_idle_ms = Some(30_000);
    assert_eq!(judge(&ax(SAFARI, "AXButton", "AXPress"), &env), Verdict::Allowed);
}

#[test]
fn missing_grants_ask_in_the_foreground_and_denials_refuse() {
    let mut env = granted();
    env.automation.clear();
    env.accessibility = None;
    let routes = vec![
        Route::Scripting {
            app: SAFARI.into(),
            verb: ScriptVerb::CloseAllTabs,
        },
        ax(SAFARI, "AXButton", "AXPress"),
        key(SAFARI, "cmd+l"), // refused for its own reason; doesn't add a grant
    ];
    let (d, _) = choose(&routes, &env);
    assert_eq!(
        d,
        Decision::Ask {
            grants: vec![Grant::Automation { app: SAFARI.into() }, Grant::Accessibility]
        }
    );

    env.files = Some(Consent::Denied);
    let (d, v) = choose(&[Route::Files(FsOp::Trash { src: r("f") })], &env);
    assert_eq!(d, Decision::Refuse);
    assert_eq!(refused(&v[0]), Some(&Refusal::Denied { grant: Grant::Files }));
}

#[test]
fn a_quarantined_rung_falls_through_to_the_next_one() {
    let mut env = granted();
    env.quarantined.insert((SAFARI.to_string(), Rung::Scripting));
    let routes = vec![
        Route::Scripting {
            app: SAFARI.into(),
            verb: ScriptVerb::CloseAllTabs,
        },
        ax(SAFARI, "AXButton", "AXPress"),
    ];
    let (d, v) = choose(&routes, &env);
    assert_eq!(
        d,
        Decision::Execute {
            index: 1,
            rung: Rung::AxAction
        }
    );
    assert_eq!(refused(&v[0]), Some(&Refusal::Quarantined));
}

#[test]
fn a_cold_launch_is_never_silent() {
    // Measured on macOS 26 CI: opening a document in a TextEdit that wasn't running activated it despite
    // `activates = false`. So workspace routes only run against running apps; a cold launch asks.
    let open = |app: &str| {
        Route::Workspace(WorkspaceOp::OpenUrl {
            app: app.into(),
            url: r("u"),
        })
    };
    let env = granted();
    assert_eq!(judge(&open(SAFARI), &env), Verdict::Allowed);
    assert_eq!(
        choose(&[open("com.google.Chrome")], &env).0,
        Decision::Ask {
            grants: vec![Grant::Foreground {
                app: "com.google.Chrome".into()
            }]
        }
    );
}

#[test]
fn routes_round_trip_as_tagged_json_without_content() {
    let route = Route::Files(FsOp::MoveInto {
        src: r("c:1"),
        dst_dir: r("c:2"),
    });
    let s = serde_json::to_string(&route).unwrap();
    assert_eq!(
        s,
        r#"{"route":"files","op":"move_into","src":"c:1","dst_dir":"c:2"}"#
    );
    assert_eq!(serde_json::from_str::<Route>(&s).unwrap(), route);
}
