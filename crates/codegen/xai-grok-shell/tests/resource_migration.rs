use std::fs;

use xai_grok_shell::resource_migration::{MigrationOptions, migrate_resources};

#[test]
fn fixture_migration_keeps_project_and_global_resources_separate() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let project_skill = project.join(".claude/skills/project-review");
    let global_agent_dir = home.join(".codex/agents");
    fs::create_dir_all(&project_skill).unwrap();
    fs::create_dir_all(&global_agent_dir).unwrap();
    fs::write(
        project_skill.join("SKILL.md"),
        "---\nname: project-review\ndescription: fixture\n---\nReview.\n",
    )
    .unwrap();
    fs::write(
        global_agent_dir.join("helper.md"),
        "---\nname: helper\ndescription: fixture\n---\nHelp.\n",
    )
    .unwrap();

    let report = migrate_resources(&MigrationOptions {
        cwd: project.clone(),
        home: home.clone(),
        dry_run: false,
        plugin_resources: Vec::new(),
    })
    .unwrap();

    assert_eq!(report.copied, 2);
    assert!(
        project
            .join(".bandicot/skills/project-review/SKILL.md")
            .is_file()
    );
    assert!(home.join(".bandicot/agents/helper.md").is_file());
    assert!(!project.join(".bandicot/agents/helper.md").exists());
    assert!(!home.join(".bandicot/skills/project-review").exists());
    assert!(
        project_skill.join("SKILL.md").is_file(),
        "source must remain"
    );
    assert!(
        global_agent_dir.join("helper.md").is_file(),
        "source must remain"
    );
}
