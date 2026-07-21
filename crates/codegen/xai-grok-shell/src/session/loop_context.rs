use std::path::{Path, PathBuf};

pub(crate) async fn augment_loop_prompt(cwd: &str, prompt: &str) -> String {
    augment_loop_prompt_from(Path::new(cwd), dirs::home_dir().as_deref(), prompt).await
}

async fn augment_loop_prompt_from(cwd: &Path, home: Option<&Path>, prompt: &str) -> String {
    let mut paths = Vec::new();
    if let Some(home) = home {
        paths.push(home.join(".bandicot").join("loop.md"));
    }
    let mut project_paths: Vec<PathBuf> = cwd
        .ancestors()
        .map(|ancestor| ancestor.join(".bandicot").join("loop.md"))
        .collect();
    project_paths.reverse();
    for path in project_paths {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }

    let mut sections = Vec::new();
    for path in paths {
        if let Ok(content) = tokio::fs::read_to_string(&path).await
            && !content.trim().is_empty()
        {
            sections.push(format!("## From: {}\n{}", path.display(), content.trim()));
        }
    }
    if sections.is_empty() {
        prompt.to_string()
    } else {
        format!(
            "{prompt}\n\n<loop_instructions>\n{}\n</loop_instructions>",
            sections.join("\n\n")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_global_then_project_loop_instructions() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = home.join("repo").join("nested");
        tokio::fs::create_dir_all(home.join(".bandicot"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.join(".bandicot"))
            .await
            .unwrap();
        tokio::fs::write(home.join(".bandicot/loop.md"), "global guidance")
            .await
            .unwrap();
        tokio::fs::write(project.join(".bandicot/loop.md"), "project guidance")
            .await
            .unwrap();

        let prompt = augment_loop_prompt_from(&project, Some(&home), "check deploy").await;
        assert!(prompt.starts_with("check deploy"));
        assert!(prompt.contains("global guidance"));
        assert!(prompt.contains("project guidance"));
        assert!(prompt.find("global guidance") < prompt.find("project guidance"));
        assert_eq!(prompt.matches("global guidance").count(), 1);
    }
}
