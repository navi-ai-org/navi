from pathlib import Path
import re
import shutil

# Always re-apply core snapshots first (workspace keeps getting reverted).
for src, dst in [
    ("/tmp/navi_skill_store.rs", "crates/navi-core/src/skills/store.rs"),
    ("/tmp/navi_skill_mod.rs", "crates/navi-core/src/skills/mod.rs"),
    ("/tmp/navi_skill_builtin.rs", "crates/navi-core/src/skills/builtin.rs"),
    ("/tmp/navi_prompt.rs", "crates/navi-core/src/prompt.rs"),
    ("/tmp/navi_runtime.rs", "crates/navi-core/src/runtime/mod.rs"),
    ("/tmp/navi_turn.rs", "crates/navi-core/src/turn/mod.rs"),
    ("/tmp/navi_config_types.rs", "crates/navi-core/src/config/types.rs"),
]:
    if Path(src).exists():
        shutil.copyfile(src, dst)
        print("restored", dst, "->", open(dst).readline().strip()[:70])

# skill_manage list replace
p = Path("crates/navi-core/src/tool/builtin/skill_manage.rs")
text = p.read_text()
list_impl = r'''impl Tool for SkillListTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "skill_list",
            "List skill catalog entries. Without `pool`: root skills + skill pools (folders). With `pool`: open that pool and list member skills (metadata only, no instruction bodies).",
            ToolKind::Read,
            helpers::json_schema(
                &[(
                    "pool",
                    "Optional pool id to open (like listing a folder). Omit for top-level catalog.",
                )],
                &[],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let config = shared_config(&self.config);
        let pool = invocation
            .input
            .get("pool")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if let Some(pool_id) = pool {
            let mut members = Vec::new();
            for s in crate::skills::builtin_skills() {
                if s.pool.as_deref() == Some(pool_id) {
                    members.push(s);
                }
            }
            if let Ok(store) =
                crate::skills::SkillStore::open_with_project(&self.data_dir, &self.project_dir)
            {
                if let Ok(stored) = store.list_pool_skills(pool_id) {
                    members.extend(stored);
                }
            }
            members.sort_by(|a, b| a.id.cmp(&b.id));
            members.dedup_by(|a, b| a.id == b.id);
            let items: Vec<_> = members
                .into_iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "name": s.name,
                        "description": s.description,
                        "pool": s.pool,
                        "allow_tools": s.allow_tools,
                        "tags": s.tags,
                        "harness": s.harness,
                        "source": format!("{:?}", s.source).to_lowercase(),
                        "kind": "skill",
                    })
                })
                .collect();
            return Ok(helpers::ok(
                invocation.id,
                json!({
                    "pool": pool_id,
                    "skills": items,
                    "count": items.len(),
                    "kind": "pool_listing",
                }),
            ));
        }

        let catalog = crate::skills::discover_catalog_entries(
            &config.skills,
            &self.project_dir,
            &self.data_dir,
        )?;
        let pools: Vec<_> = catalog
            .pools
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "description": p.description,
                    "skill_count": p.skill_count,
                    "kind": "pool",
                })
            })
            .collect();
        let skills: Vec<_> = catalog
            .root_skills
            .into_iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "name": s.name,
                    "description": s.description,
                    "allow_tools": s.allow_tools,
                    "tags": s.tags,
                    "harness": s.harness,
                    "source": format!("{:?}", s.source).to_lowercase(),
                    "kind": "skill",
                })
            })
            .collect();
        Ok(helpers::ok(
            invocation.id,
            json!({
                "pools": pools,
                "skills": skills,
                "pool_count": pools.len(),
                "skill_count": skills.len(),
                "kind": "catalog",
            }),
        ))
    }
}'''
text2, n = re.subn(
    r"impl Tool for SkillListTool \{.*?^\}",
    list_impl,
    text,
    count=1,
    flags=re.S | re.M,
)
print("list", n)
text = text2
if "Optional skill pool folder id" not in text:
    text = text.replace(
        '''                    (
                        "harness",
                        "When true, materialize a harness pack (loop.toml/graph.toml) after saving.",
                    ),
                ],
                &["name", "instructions"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let name = invocation''',
        '''                    (
                        "pool",
                        "Optional skill pool folder id (e.g. `navi`). Creates the pool if needed.",
                    ),
                    (
                        "harness",
                        "When true, materialize a harness pack (loop.toml/graph.toml) after saving.",
                    ),
                ],
                &["name", "instructions"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let name = invocation''',
    )
text = re.sub(
    r'harness: parse_bool\(invocation\.input\.get\("harness"\)\),\n\s*pool: None,\n',
    '''harness: parse_bool(invocation.input.get("harness")),
            pool: invocation
                .input
                .get("pool")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
''',
    text,
)
p.write_text(text)
print("pool_listing", "pool_listing" in text)

for f in [
    "crates/navi-core/src/harness_pack/apply.rs",
    "crates/navi-core/src/harness_pack/materialize.rs",
    "crates/navi-core/src/turn/tests.rs",
]:
    t = Path(f).read_text()
    t2 = re.sub(
        r"(harness:\s*(?:true|false),)\n(?!\s*pool:)",
        r"\1\n            pool: None,\n",
        t,
    )
    Path(f).write_text(t2)

for f in [
    "crates/navi-core/src/session.rs",
    "crates/navi-core/src/turn/tests.rs",
    "crates/navi-core/src/memory/tests.rs",
    "crates/navi-core/src/tool/builtin/subagent.rs",
]:
    t = Path(f).read_text()
    t2 = re.sub(
        r"(available_skills:\s*(?:std::sync::)?Arc::new\(std::sync::Mutex::new\(Vec::new\(\)\)\),)\n(\s*)(active_skills:)",
        r"\1\n\2skill_pools: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),\n\2\3",
        t,
    )
    Path(f).write_text(t2)

print(open("crates/navi-core/src/skills/store.rs").readline().strip())
print("find", "find_skill" in open("crates/navi-core/src/tool/builtin/skill_tool.rs").read())
print("turn pools", "skill_pools" in open("crates/navi-core/src/turn/mod.rs").read())
print("pool field", "pub pool: Option" in open("crates/navi-core/src/skills/mod.rs").read())
