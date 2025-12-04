use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use serde::Deserialize;
use serde_yaml::Value;
use std::fs;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "pin-and-bump")]
#[command(about = "Pin GitHub Actions to commit SHAs and optionally update to latest versions")]
struct Args {
    /// Update to latest versions
    #[arg(long)]
    update: bool,

    /// Path to repository (defaults to current directory)
    #[arg(short, long, default_value = ".")]
    path: PathBuf,
}

#[derive(Debug)]
struct ActionReference {
    owner: String,
    repo: String,
    reference: String,
}

#[derive(Debug, Deserialize)]
struct GitHubTag {
    object: GitHubObject,
}

#[derive(Debug, Deserialize)]
struct GitHubObject {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GitHubCommit {
    sha: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Find all workflow files
    let workflow_pattern = args.path.join(".github/workflows/*.yaml");
    let workflow_pattern_yml = args.path.join(".github/workflows/*.yml");

    let mut workflow_files = Vec::new();

    for pattern in &[workflow_pattern, workflow_pattern_yml] {
        for entry in
            glob::glob(pattern.to_str().unwrap()).context("Failed to read workflow pattern")?
        {
            match entry {
                Ok(path) => workflow_files.push(path),
                Err(e) => eprintln!("Error reading path: {}", e),
            }
        }
    }

    if workflow_files.is_empty() {
        println!("No workflow files found in .github/workflows/");
        return Ok(());
    }

    // Process each workflow file
    for workflow_file in workflow_files {
        process_workflow_file(&workflow_file, args.update)?;
    }

    Ok(())
}

fn process_workflow_file(file_path: &PathBuf, update: bool) -> Result<()> {
    let content =
        fs::read_to_string(file_path).context(format!("Failed to read file: {:?}", file_path))?;

    let action_refs = find_action_references(&content)?;

    if action_refs.is_empty() {
        return Ok(());
    }

    println!("\nProcessing: {}", file_path.display());

    let mut updated_content = content.clone();
    let mut changes = Vec::new();

    for action_ref in action_refs {
        match resolve_reference(&action_ref, update) {
            Ok((sha, version_tag)) => {
                let old_uses = format!(
                    "{}/{}@{}",
                    action_ref.owner, action_ref.repo, action_ref.reference
                );
                let new_uses = format!(
                    "{}/{}@{} # {}",
                    action_ref.owner, action_ref.repo, sha, version_tag
                );

                // Only update if it's not already pinned to this SHA
                if !action_ref.reference.starts_with(&sha[..7]) {
                    updated_content = updated_content.replace(
                        &format!("uses: {}", old_uses),
                        &format!("uses: {}", new_uses),
                    );
                    changes.push((old_uses, new_uses));
                }
            }
            Err(e) => {
                eprintln!(
                    "  Error resolving {}/{}@{}: {}",
                    action_ref.owner, action_ref.repo, action_ref.reference, e
                );
            }
        }
    }

    if !changes.is_empty() {
        fs::write(file_path, updated_content)
            .context(format!("Failed to write file: {:?}", file_path))?;

        for (old, new) in changes {
            println!("  {} {} {}", old.red(), "→".bright_white(), new.green());
        }
    }

    Ok(())
}

fn find_action_references(content: &str) -> Result<Vec<ActionReference>> {
    let yaml: Value = serde_yaml::from_str(content).context("Failed to parse YAML")?;

    let mut refs = Vec::new();
    extract_uses_from_value(&yaml, &mut refs);
    Ok(refs)
}

fn extract_uses_from_value(value: &Value, refs: &mut Vec<ActionReference>) {
    match value {
        Value::Mapping(map) => {
            for (key, val) in map {
                if let Some(key_str) = key.as_str() {
                    if key_str == "uses" {
                        if let Some(uses_str) = val.as_str() {
                            if let Some(action_ref) = parse_uses_string(uses_str) {
                                refs.push(action_ref);
                            }
                        }
                    }
                }
                extract_uses_from_value(val, refs);
            }
        }
        Value::Sequence(seq) => {
            for item in seq {
                extract_uses_from_value(item, refs);
            }
        }
        _ => {}
    }
}

fn parse_uses_string(uses: &str) -> Option<ActionReference> {
    // Parse "owner/repo@reference" format
    // Extract just the part before any comment
    let uses_clean = uses.split('#').next()?.trim();

    let parts: Vec<&str> = uses_clean.split('@').collect();
    if parts.len() != 2 {
        return None;
    }

    let reference = parts[1].trim().to_string();

    // Skip if already pinned to a SHA (40 hex chars)
    if reference.len() == 40 && reference.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let repo_parts: Vec<&str> = parts[0].split('/').collect();
    if repo_parts.len() < 2 {
        return None;
    }

    let owner = repo_parts[0].to_string();
    let repo = repo_parts[1..].join("/");

    Some(ActionReference {
        owner,
        repo,
        reference,
    })
}

fn resolve_reference(action_ref: &ActionReference, update: bool) -> Result<(String, String)> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("pin-and-bump/0.1.0")
        .build()?;

    resolve_reference_with_client(action_ref, update, &client, "https://api.github.com")
}

fn resolve_reference_with_client(
    action_ref: &ActionReference,
    update: bool,
    client: &reqwest::blocking::Client,
    base_url: &str,
) -> Result<(String, String)> {
    if update {
        // Get latest release or tag
        let latest_url = format!(
            "{}/repos/{}/{}/releases/latest",
            base_url, action_ref.owner, action_ref.repo
        );

        #[derive(Deserialize)]
        struct Release {
            tag_name: String,
        }

        let response = client.get(&latest_url).send();

        match response {
            Ok(resp) if resp.status().is_success() => {
                let release: Release = resp.json()?;
                let tag = &release.tag_name;

                // Now get the SHA for this tag
                let sha = get_sha_for_ref_with_base(
                    client,
                    base_url,
                    &action_ref.owner,
                    &action_ref.repo,
                    tag,
                )?;
                return Ok((sha, tag.clone()));
            }
            _ => {
                // Fall back to getting SHA for the current reference
            }
        }
    }

    // Get SHA for the current reference
    let sha = get_sha_for_ref_with_base(
        client,
        base_url,
        &action_ref.owner,
        &action_ref.repo,
        &action_ref.reference,
    )?;
    Ok((sha, action_ref.reference.clone()))
}

fn get_sha_for_ref_with_base(
    client: &reqwest::blocking::Client,
    base_url: &str,
    owner: &str,
    repo: &str,
    ref_name: &str,
) -> Result<String> {
    let url = format!(
        "{}/repos/{}/{}/git/ref/tags/{}",
        base_url, owner, repo, ref_name
    );

    let response = client.get(&url).send()?;

    if response.status().is_success() {
        let tag: GitHubTag = response.json()?;

        // Tags can point to tag objects or commits directly
        // If it's a tag object, we need to dereference it
        let commit_sha = if tag.object.sha.len() == 40 {
            // Try to get the commit this tag points to
            let commit_url = format!(
                "{}/repos/{}/{}/git/tags/{}",
                base_url, owner, repo, tag.object.sha
            );

            #[derive(Deserialize)]
            struct TagObject {
                object: GitHubObject,
            }

            match client.get(&commit_url).send() {
                Ok(resp) if resp.status().is_success() => {
                    let tag_obj: TagObject = resp.json()?;
                    tag_obj.object.sha
                }
                _ => tag.object.sha,
            }
        } else {
            tag.object.sha
        };

        Ok(commit_sha)
    } else {
        // Try as a branch or direct commit reference
        let url = format!("{}/repos/{}/{}/commits/{}", base_url, owner, repo, ref_name);

        let response = client.get(&url).send()?;

        if response.status().is_success() {
            let commit: GitHubCommit = response.json()?;
            Ok(commit.sha)
        } else {
            anyhow::bail!("Could not resolve reference: HTTP {}", response.status())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_action_references() {
        let input = r#"
jobs:
  test:
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-go@v5
      - uses: docker/setup-buildx-action@v3.0.0
      - uses: owner/repo@abc123def456789012345678901234567890abcd
      - name: Something
"#;

        let refs = find_action_references(input).unwrap();

        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].owner, "actions");
        assert_eq!(refs[0].repo, "checkout");
        assert_eq!(refs[0].reference, "v4");

        assert_eq!(refs[1].owner, "actions");
        assert_eq!(refs[1].repo, "setup-go");
        assert_eq!(refs[1].reference, "v5");

        assert_eq!(refs[2].owner, "docker");
        assert_eq!(refs[2].repo, "setup-buildx-action");
        assert_eq!(refs[2].reference, "v3.0.0");
    }

    #[test]
    fn test_skips_already_pinned_shas() {
        let input = r#"
jobs:
  test:
    steps:
      - uses: actions/checkout@8ade135a41bc03ea155e62e844d188df1ea18608 # v4
"#;

        let refs = find_action_references(input).unwrap();
        assert_eq!(refs.len(), 0);
    }

    #[test]
    fn test_resolve_reference_with_mocked_api() {
        use mockito::Server;

        let mut server = Server::new();

        // Mock the tag resolution endpoint
        let _mock = server
            .mock("GET", "/repos/actions/checkout/git/ref/tags/v4")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"object": {"sha": "8ade135a41bc03ea155e62e844d188df1ea18608"}}"#)
            .create();

        let action_ref = ActionReference {
            owner: "actions".to_string(),
            repo: "checkout".to_string(),
            reference: "v4".to_string(),
        };

        let client = reqwest::blocking::Client::new();
        let result = resolve_reference_with_client(&action_ref, false, &client, &server.url());

        assert!(result.is_ok());
        let (sha, tag) = result.unwrap();
        assert_eq!(sha, "8ade135a41bc03ea155e62e844d188df1ea18608");
        assert_eq!(tag, "v4");
    }

    #[test]
    fn test_full_workflow_transformation_with_mocked_api() {
        use mockito::Server;
        use std::fs;
        use tempfile::TempDir;

        let mut server = Server::new();

        // Mock tag resolutions
        let _mock1 = server
            .mock("GET", "/repos/actions/checkout/git/ref/tags/v4")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"object": {"sha": "8ade135a41bc03ea155e62e844d188df1ea18608"}}"#)
            .create();

        let _mock2 = server
            .mock("GET", "/repos/actions/setup-go/git/ref/tags/v5")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"object": {"sha": "0a12ed9d6a9990640e88f7f159f6c4bc9925b9b2"}}"#)
            .create();

        let before = r#"name: Test
on: [push]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-go@v5
      - run: go test
"#;

        // Create temporary workflow file
        let temp_dir = TempDir::new().unwrap();
        let workflows_dir = temp_dir.path().join(".github/workflows");
        fs::create_dir_all(&workflows_dir).unwrap();
        let workflow_file = workflows_dir.join("test.yaml");
        fs::write(&workflow_file, before).unwrap();

        // Process the file
        let content = fs::read_to_string(&workflow_file).unwrap();
        let action_refs = find_action_references(&content).unwrap();

        assert_eq!(action_refs.len(), 2);

        let client = reqwest::blocking::Client::new();
        let mut updated_content = content.clone();

        for action_ref in action_refs {
            let result = resolve_reference_with_client(&action_ref, false, &client, &server.url());
            assert!(result.is_ok());

            let (sha, version_tag) = result.unwrap();
            let old_uses = format!(
                "{}/{}@{}",
                action_ref.owner, action_ref.repo, action_ref.reference
            );
            let new_uses = format!(
                "{}/{}@{} # {}",
                action_ref.owner, action_ref.repo, sha, version_tag
            );

            updated_content = updated_content.replace(
                &format!("uses: {}", old_uses),
                &format!("uses: {}", new_uses),
            );
        }

        // Verify the transformation
        assert!(
            updated_content
                .contains("actions/checkout@8ade135a41bc03ea155e62e844d188df1ea18608 # v4")
        );
        assert!(
            updated_content
                .contains("actions/setup-go@0a12ed9d6a9990640e88f7f159f6c4bc9925b9b2 # v5")
        );
        assert!(!updated_content.contains("actions/checkout@v4"));
        assert!(!updated_content.contains("actions/setup-go@v5"));
    }

    #[test]
    fn test_resolve_reference_with_update_flag() {
        use mockito::Server;

        let mut server = Server::new();

        // Mock the latest release endpoint
        let _mock_release = server
            .mock("GET", "/repos/actions/checkout/releases/latest")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"tag_name": "v4.2.0"}"#)
            .create();

        // Mock the tag resolution for the latest version
        let _mock_tag = server
            .mock("GET", "/repos/actions/checkout/git/ref/tags/v4.2.0")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"object": {"sha": "11111111111111111111111111111111111111ab"}}"#)
            .create();

        let action_ref = ActionReference {
            owner: "actions".to_string(),
            repo: "checkout".to_string(),
            reference: "v4".to_string(),
        };

        let client = reqwest::blocking::Client::new();
        let result = resolve_reference_with_client(&action_ref, true, &client, &server.url());

        assert!(result.is_ok());
        let (sha, tag) = result.unwrap();
        assert_eq!(sha, "11111111111111111111111111111111111111ab");
        assert_eq!(tag, "v4.2.0"); // Should be updated to latest version
    }

    #[test]
    fn test_resolve_reference_update_fallback() {
        use mockito::Server;

        let mut server = Server::new();

        // Mock latest release to fail (no release)
        let _mock_release = server
            .mock("GET", "/repos/actions/checkout/releases/latest")
            .with_status(404)
            .create();

        // Mock fallback to current tag
        let _mock_tag = server
            .mock("GET", "/repos/actions/checkout/git/ref/tags/v4")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"object": {"sha": "8ade135a41bc03ea155e62e844d188df1ea18608"}}"#)
            .create();

        let action_ref = ActionReference {
            owner: "actions".to_string(),
            repo: "checkout".to_string(),
            reference: "v4".to_string(),
        };

        let client = reqwest::blocking::Client::new();
        let result = resolve_reference_with_client(&action_ref, true, &client, &server.url());

        assert!(result.is_ok());
        let (sha, tag) = result.unwrap();
        assert_eq!(sha, "8ade135a41bc03ea155e62e844d188df1ea18608");
        assert_eq!(tag, "v4"); // Should fall back to current reference
    }

    #[test]
    fn test_parse_uses_string() {
        let test_cases = vec![
            ("actions/checkout@v4", Some(("actions", "checkout", "v4"))),
            (
                "docker/build-push-action@v5.1.0",
                Some(("docker", "build-push-action", "v5.1.0")),
            ),
            (
                "github/codeql-action/analyze@v2",
                Some(("github", "codeql-action/analyze", "v2")),
            ),
            (
                "actions/checkout@8ade135a41bc03ea155e62e844d188df1ea18608",
                None,
            ), // Skip 40-char SHA
            (
                "actions/checkout@abc123 # v4",
                Some(("actions", "checkout", "abc123")),
            ), // Comment stripped
        ];

        for (input, expected) in test_cases {
            let result = parse_uses_string(input);
            if let Some((owner, repo, reference)) = expected {
                let action_ref = result.unwrap();
                assert_eq!(action_ref.owner, owner);
                assert_eq!(action_ref.repo, repo);
                assert_eq!(action_ref.reference, reference);
            } else {
                assert!(result.is_none());
            }
        }
    }

    #[test]
    fn test_nested_workflow_structure() {
        let input = r#"
name: Complex Workflow
on: [push, pull_request]
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-go@v5
"#;

        let refs = find_action_references(input).unwrap();
        assert_eq!(refs.len(), 4);
        assert_eq!(refs[0].owner, "actions");
        assert_eq!(refs[0].repo, "checkout");
    }
}
