//! Live smoke: exercise every registered Boris tool body (direct `execute`).
//! Run: `cargo test -p boris-agent --test tool_live_smoke -- --nocapture`

use std::sync::{Arc, Mutex};
use std::time::Instant;

use boris_agent::memory::profile::UserProfile;
use boris_agent::tool::Tool;
use boris_agent::tools::clipboard::{ClipboardGetTool, ClipboardSetTool};
use boris_agent::tools::files::{FsRoots, ListDirTool, ReadFileTool, WriteFileTool};
use boris_agent::tools::notes::{RecallNotesTool, RememberNoteTool};
use boris_agent::tools::open_tool::{OpenPathTool, OpenUrlTool};
use boris_agent::tools::profile::{
    GetUserContextTool, SaveUserFactTool, UpdateUserProfileTool,
};
use boris_agent::tools::shell::RunCommandTool;
use boris_agent::tools::system::GetSystemInfoTool;
use boris_agent::tools::time::{GetDateTool, GetTimeTool};
use boris_agent::tools::todo::{TodoReadTool, TodoWriteTool};
use boris_agent::tools::web::{WebFetchTool, WebSearchTool};
use serde_json::json;

struct SmokeResult {
    name: &'static str,
    ok: bool,
    detail: String,
    ms: u128,
}

async fn run_one(name: &'static str, f: impl std::future::Future<Output = Result<String, String>>) -> SmokeResult {
    let t0 = Instant::now();
    match f.await {
        Ok(out) => {
            let preview: String = out.chars().take(160).collect();
            SmokeResult {
                name,
                ok: true,
                detail: preview.replace('\n', " | "),
                ms: t0.elapsed().as_millis(),
            }
        }
        Err(e) => SmokeResult {
            name,
            ok: false,
            detail: e,
            ms: t0.elapsed().as_millis(),
        },
    }
}

#[tokio::test]
async fn live_smoke_all_boris_tools() {
    let root = std::env::temp_dir().join(format!(
        "boris-tool-smoke-{}",
        std::process::id()
    ));
    let sandbox = root.join("sandbox");
    let memory = root.join("memory");
    std::fs::create_dir_all(&sandbox).unwrap();
    std::fs::create_dir_all(&memory).unwrap();

    let notes_path = memory.join("notes.jsonl");
    let profile_path = memory.join("profile.json");
    let roots = FsRoots {
        sandbox: sandbox.clone(),
        data: vec![memory.clone()],
        allow_read: vec![],
        allow_write: vec![],
    };

    let mut results: Vec<SmokeResult> = Vec::new();

    // ── time ──────────────────────────────────────────────────────────────
    results.push(
        run_one("get_time", async {
            GetTimeTool
                .execute(json!({}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );
    results.push(
        run_one("get_date", async {
            GetDateTool
                .execute(json!({}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── notes ─────────────────────────────────────────────────────────────
    results.push(
        run_one("remember_note", async {
            RememberNoteTool::new(notes_path.clone())
                .execute(json!({"note": "smoke test note from tool_live_smoke"}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );
    results.push(
        run_one("recall_notes", async {
            RecallNotesTool::new(notes_path.clone())
                .execute(json!({"limit": 5}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── profile ───────────────────────────────────────────────────────────
    let profile = Arc::new(Mutex::new(UserProfile::default()));
    results.push(
        run_one("save_user_fact", async {
            SaveUserFactTool::with_path(profile.clone(), profile_path.clone())
                .execute(json!({"fact": "likes smoke tests", "category": "preference"}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );
    results.push(
        run_one("update_user_profile", async {
            UpdateUserProfileTool::with_path(profile.clone(), profile_path.clone())
                .execute(json!({"preferred_name": "Smoke Tester", "address_as": "Tester"}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );
    results.push(
        run_one("get_user_context", async {
            GetUserContextTool::new(profile.clone())
                .execute(json!({}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── system ────────────────────────────────────────────────────────────
    results.push(
        run_one("get_system_info", async {
            GetSystemInfoTool::new(root.display().to_string())
                .execute(json!({}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── clipboard ─────────────────────────────────────────────────────────
    results.push(
        run_one("clipboard_set", async {
            ClipboardSetTool
                .execute(json!({"text": "boris-smoke-clipboard"}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );
    results.push(
        run_one("clipboard_get", async {
            ClipboardGetTool
                .execute(json!({}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── todos ─────────────────────────────────────────────────────────────
    results.push(
        run_one("todo_write", async {
            TodoWriteTool::new(&sandbox)
                .execute(json!({
                    "items": [
                        {"id": "1", "content": "smoke item", "status": "pending"}
                    ]
                }))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );
    results.push(
        run_one("todo_read", async {
            TodoReadTool::new(&sandbox)
                .execute(json!({}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── filesystem ────────────────────────────────────────────────────────
    results.push(
        run_one("write_file", async {
            WriteFileTool::new(roots.clone())
                .execute(json!({
                    "path": sandbox.join("hello.txt").to_string_lossy(),
                    "content": "hello from boris smoke\n"
                }))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );
    results.push(
        run_one("read_file", async {
            ReadFileTool::new(roots.clone())
                .execute(json!({
                    "path": sandbox.join("hello.txt").to_string_lossy()
                }))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );
    results.push(
        run_one("list_dir", async {
            ListDirTool::new(roots.clone())
                .execute(json!({
                    "path": sandbox.to_string_lossy()
                }))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── shell ─────────────────────────────────────────────────────────────
    results.push(
        run_one("run_command", async {
            RunCommandTool::new(vec![sandbox.clone()], sandbox.clone())
                .execute(json!({
                    "command": "Write-Output 'shell-smoke-ok'",
                    "cwd": sandbox.to_string_lossy()
                }))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── web ───────────────────────────────────────────────────────────────
    results.push(
        run_one("web_search", async {
            WebSearchTool::new()
                .map_err(|e| e.message)?
                .execute(json!({"query": "rust programming language", "limit": 3}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );
    results.push(
        run_one("web_fetch", async {
            WebFetchTool::new()
                .map_err(|e| e.message)?
                .execute(json!({"url": "https://example.com"}))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── open (side-effect tools): validate path open on sandbox file only ─
    // Skip open_url success path (would launch a browser). Still check denial.
    results.push(
        run_one("open_url (invalid scheme denied)", async {
            match OpenUrlTool
                .execute(json!({"url": "ftp://example.com"}))
                .await
            {
                Err(e) if e.message.contains("http") => {
                    Ok(format!("correctly denied: {}", e.message))
                }
                Ok(s) => Err(format!("expected deny, got ok: {s}")),
                Err(e) => Err(format!("unexpected error: {}", e.message)),
            }
        })
        .await,
    );
    results.push(
        run_one("open_path", async {
            // Opening may launch Explorer/editor; still verifies OS handoff works.
            OpenPathTool::new(vec![sandbox.clone()])
                .execute(json!({
                    "path": sandbox.join("hello.txt").to_string_lossy()
                }))
                .await
                .map_err(|e| e.message)
        })
        .await,
    );

    // ── report ────────────────────────────────────────────────────────────
    println!("\n======== BORIS TOOL LIVE SMOKE ========");
    let mut failed = 0usize;
    for r in &results {
        let status = if r.ok { "PASS" } else { "FAIL" };
        if !r.ok {
            failed += 1;
        }
        println!("[{status}] {:<36} {:>5}ms  {}", r.name, r.ms, r.detail);
    }
    println!(
        "======== {}/{} passed ========\n",
        results.len() - failed,
        results.len()
    );

    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(
        failed, 0,
        "{failed} Boris tool(s) failed live smoke — see output above"
    );
}
