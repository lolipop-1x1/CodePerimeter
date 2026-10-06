use codeperimeter::native::{DirectoryChoice, JobStatus, ServiceAction, ServiceStatus};
use codeperimeter::service::ServicePlan;
use std::path::Path;

#[test]
fn web_service_actions_are_a_closed_password_free_contract() {
    for action in ["install", "start", "pause", "resume", "uninstall"] {
        let value = format!("\"{action}\"");
        let parsed: ServiceAction = serde_json::from_str(&value).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), value);
    }
    for rejected in ["stop", "shell", "sudo", "custom-command"] {
        assert!(serde_json::from_str::<ServiceAction>(&format!("\"{rejected}\"")).is_err());
    }
}

#[test]
fn service_status_keeps_install_collection_and_desktop_notification_separate() {
    let status = ServiceStatus {
        uid: 1,
        installed: true,
        installed_binary_trusted: true,
        collector: JobStatus {
            disabled: Some(true),
            ..JobStatus::default()
        },
        analyzer: JobStatus {
            loaded: true,
            running: true,
            ..JobStatus::default()
        },
        notification: JobStatus::default(),
        paused: Some(true),
        status_error: None,
    };
    let value = serde_json::to_value(status).unwrap();
    assert_eq!(value["paused"], true);
    assert_eq!(value["analyzer"]["running"], true);
    assert_eq!(value["collector"]["loaded"], false);
    assert_eq!(value["notification"]["loaded"], false);
    assert!(value.get("protected").is_none());
}

#[test]
fn lifecycle_plan_uses_fixed_jobs_and_pause_does_not_change_plan_roles() {
    let plan = ServicePlan::new("nobody", Path::new("/synthetic/codeperimeter")).unwrap();
    assert_eq!(
        plan.jobs
            .iter()
            .map(|job| job.argv[1].as_str())
            .collect::<Vec<_>>(),
        vec!["collector", "daemon", "notify"]
    );
    assert!(
        plan.jobs
            .iter()
            .all(|job| job.argv.iter().all(|argument| argument != "password"))
    );
    assert_eq!(
        serde_json::to_value(DirectoryChoice {
            path: None,
            cancelled: true
        })
        .unwrap(),
        serde_json::json!({"path":null,"cancelled":true})
    );
}
