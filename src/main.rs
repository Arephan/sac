use anyhow::{bail, Result};
use clap::Parser;
use sac::apply::{assert_safe_out_root, render};
use sac::cli::{Cli, Cmd};
use sac::core::{desired_state, import_live, load_instance, resolve_instance, ImportArgs};
use sac::ctx::real_home;
use sac::importers::load_importers;
use sac::init::init;
use sac::layers::LAYERS;
use sac::lint::{default_targets, format_lint, instance_allow, instance_terms, lint};
use sac::live::{apply_live, LiveOptions};
use sac::output::{counts_block, green, paint_statuses, red};
use sac::plan::{format_plan, plan};
use sac::policy::{format_policy_results, load_checks, run_policies, CheckCtx};
use sac::redact::redact;
use sac::schema::validate;
use sac::util::now_iso;
use sac::yaml::{read_yaml, to_yaml};
use serde_json::json;
use std::path::{Path, PathBuf};

fn rel(p: &Path, cwd: &Path) -> String {
    p.strip_prefix(cwd)
        .map(|r| {
            let s = r.to_string_lossy().to_string();
            if s.is_empty() {
                ".".to_string()
            } else {
                s
            }
        })
        .unwrap_or_else(|_| p.to_string_lossy().to_string())
}

fn main() {
    let code = match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{} {e:#}", red("error:"));
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    let home: PathBuf = cli.home.clone().unwrap_or_else(real_home);

    match &cli.cmd {
        Cmd::Init { dir, name, force } => {
            let r = init(dir, name.as_deref(), *force, &home)?;
            println!(
                "initialized instance \"{}\" in {}: {}",
                r.instance,
                rel(&r.dir, &cwd),
                r.files.join(", ")
            );
            println!("next: sac validate -i {}", rel(&r.dir, &cwd));
            return Ok(0);
        }
        Cmd::Layers => {
            for l in LAYERS.iter() {
                println!("{:<13} {}", l.id, l.summary);
            }
            return Ok(0);
        }
        Cmd::Importers => {
            let dir = resolve_instance(cli.instance.as_deref(), &cwd)
                .ok()
                .map(|i| i.dir);
            for i in load_importers(dir.as_deref())? {
                println!(
                    "{:<20} [{}] {}",
                    i.id(),
                    i.layers().join(","),
                    i.description()
                );
            }
            return Ok(0);
        }
        Cmd::Lint {
            paths,
            instance_root,
        } => {
            let root = cwd.clone();
            let iroot = instance_root
                .clone()
                .unwrap_or_else(|| root.join("instance"));
            let targets: Vec<PathBuf> = if paths.is_empty() {
                default_targets(&root)
            } else {
                paths
                    .iter()
                    .map(|p| {
                        if p.is_absolute() {
                            p.clone()
                        } else {
                            root.join(p)
                        }
                    })
                    .collect()
            };
            let r = lint(
                &root,
                &targets,
                &instance_terms(&iroot),
                &instance_allow(&iroot),
            );
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                println!("{}", format_lint(&r));
            }
            return Ok(if r.findings.is_empty() { 0 } else { 1 });
        }
        _ => {}
    }

    let inst = resolve_instance(cli.instance.as_deref(), &cwd)?;

    if let Cmd::Import { dry_run } = &cli.cmd {
        let existing = if inst.file.exists() {
            read_yaml(&inst.file)?
        } else {
            json!({})
        };
        let ov = inst.dir.join("overrides.yaml");
        let overrides = if ov.exists() {
            read_yaml(&ov)?
        } else {
            json!({})
        };
        let pol = inst.dir.join("policies.yaml");
        let policies = if pol.exists() {
            read_yaml(&pol)?
        } else {
            json!({})
        };
        println!(
            "import: reading machine (read-only) for instance {}",
            rel(&inst.dir, &cwd)
        );
        let r = import_live(ImportArgs {
            inst: &inst,
            setup: &existing,
            overrides: &overrides,
            policies: &policies,
            home: cli.home.as_deref(),
            baseline: &existing,
        })?;
        for x in &r.ran {
            println!(
                "  {:<12} {}{}",
                x.status,
                x.id,
                x.error
                    .as_ref()
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default()
            );
        }
        if !r.warnings.is_empty() {
            if cli.verbose {
                for w in &r.warnings {
                    println!("  ! {w}");
                }
            } else {
                println!("  {} warning(s) (use -v)", r.warnings.len());
            }
        }
        let mut live = r.live;
        live["meta"]["generated_at"] = json!(now_iso());
        live["meta"]["generated_by"] = json!(format!("sac import {}", sac::VERSION));
        let v = validate(&live);
        println!("{}", counts_block(&live, "counts per layer:"));
        if !v.ok {
            println!("imported state fails the schema:");
            for e in v.errors.iter().take(30) {
                println!("  {e}");
            }
            return Ok(1);
        }
        if *dry_run {
            println!("dry run: nothing written");
            return Ok(0);
        }
        let header = format!(
            "# Generated by `sac import` on {}. Re-running import rewrites this file.\n# Hand-curated facts belong in overrides.yaml; policies in policies.yaml.",
            sac::util::today()
        );
        // the instance folder obeys the same protected-path rule as every other output
        assert_safe_out_root(&inst.dir, &real_home())?;
        if std::fs::symlink_metadata(&inst.file)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            bail!(
                "refusing to write through symlink {}",
                rel(&inst.file, &cwd)
            );
        }
        std::fs::write(&inst.file, to_yaml(&live, &header))?;
        println!("wrote {}", rel(&inst.file, &cwd));
        return Ok(0);
    }

    let loaded = load_instance(&inst)?;
    let desired = desired_state(&loaded);

    if let Cmd::Validate = &cli.cmd {
        let v = validate(&desired);
        println!(
            "{}",
            counts_block(&desired, &format!("validate {}", rel(&inst.file, &cwd)))
        );
        for w in &v.warnings {
            println!("  warn: {w}");
        }
        for e in &v.errors {
            println!("  error: {e}");
        }
        println!(
            "{}",
            if v.ok {
                green("  VALID")
            } else {
                red(&format!("  INVALID ({} error(s))", v.errors.len()))
            }
        );
        return Ok(if v.ok { 0 } else { 1 });
    }

    if let Cmd::Redact { out } = &cli.cmd {
        let r = redact(&desired);
        let v = validate(&r);
        if !v.ok {
            for e in &v.errors {
                println!("  error: {e}");
            }
            return Ok(1);
        }
        // the same protected-path rule as apply: never land a copy inside live config
        let safe_out = assert_safe_out_root(out, &real_home())?;
        let f = safe_out.join("setup.yaml");
        if std::fs::symlink_metadata(&f)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            bail!("refusing to write through symlink {}", rel(&f, &cwd));
        }
        std::fs::write(
            &f,
            to_yaml(
                &r,
                "# Shape-only redaction generated by `sac redact`. Every name, path, handle and text is replaced;\n# only counts, kinds, severities, scopes, dates, schedules and on/off flags are real.",
            ),
        )?;
        println!("{}", counts_block(&r, &format!("wrote {}", rel(&f, &cwd))));
        return Ok(0);
    }

    let imported = import_live(ImportArgs {
        inst: &inst,
        setup: &loaded.setup,
        overrides: &loaded.overrides,
        policies: &loaded.policies,
        home: cli.home.as_deref(),
        baseline: &desired,
    })?;
    let live = imported.live;
    let p = plan(&desired, &live);
    if cli.verbose && !cli.json {
        for w in &imported.warnings {
            println!("  ! {w}");
        }
    }

    if let Cmd::Plan = &cli.cmd {
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&p)?);
        } else {
            println!(
                "plan: {} (desired) vs machine (live{})",
                rel(&inst.file, &cwd),
                cli.home
                    .as_ref()
                    .map(|h| format!(", home={}", h.display()))
                    .unwrap_or_default()
            );
            println!("{}", format_plan(&p, cli.verbose));
        }
        return Ok(0);
    }

    let checks = load_checks(Some(&inst.dir))?;
    let out_root = match &cli.cmd {
        Cmd::Apply { out, .. } => out.clone().unwrap_or_else(|| cwd.join("out")),
        _ => cwd.join("out"),
    };
    let cctx = CheckCtx {
        home: home.clone(),
        out_root: Some(out_root.clone()),
    };

    if let Cmd::Verify { fail_on_drift } = &cli.cmd {
        let v = validate(&desired);
        let pd = run_policies(&desired, &checks, &cctx);
        let pl = run_policies(&live, &checks, &cctx);
        let drift = p.totals.add + p.totals.change + p.totals.untracked;
        if cli.json {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({ "schema": v, "plan": p.totals, "desired": pd, "live": pl })
                )?
            );
        } else {
            println!("verify {}", rel(&inst.file, &cwd));
            println!(
                "schema: {}",
                if v.ok {
                    "valid".to_string()
                } else {
                    format!("INVALID ({})", v.errors.len())
                }
            );
            for e in v.errors.iter().take(20) {
                println!("  error: {e}");
            }
            println!(
                "drift: {} to add, {} changed, {} untracked{}",
                p.totals.add,
                p.totals.change,
                p.totals.untracked,
                if drift > 0 {
                    " (run `plan` for detail, `import` to adopt live state)"
                } else {
                    ""
                }
            );
            println!("policies on desired (setup.yaml):");
            println!(
                "{}",
                paint_statuses(&format_policy_results(&pd, cli.verbose))
            );
            println!("policies on live (machine now):");
            println!(
                "{}",
                paint_statuses(&format_policy_results(&pl, cli.verbose))
            );
            let mut hard: Vec<String> = Vec::new();
            for r in pd.hard_fails().iter().chain(pl.hard_fails().iter()) {
                if !hard.contains(&r.id) {
                    hard.push(r.id.clone());
                }
            }
            if hard.is_empty() {
                println!(
                    "{}",
                    green(&format!(
                        "VERIFY OK: {} policies, 0 hard failures{}",
                        pd.results.len(),
                        if imported.warnings.is_empty() {
                            String::new()
                        } else {
                            format!(", {} import warning(s)", imported.warnings.len())
                        }
                    ))
                );
            } else {
                println!(
                    "{}",
                    red(&format!(
                        "VERIFY FAILED: hard policy failure(s): {}",
                        hard.join(", ")
                    ))
                );
            }
        }
        if !v.ok || !pd.ok || !pl.ok {
            return Ok(1);
        }
        if *fail_on_drift && drift > 0 {
            return Ok(2);
        }
        return Ok(0);
    }

    if let Cmd::Apply {
        live: live_mode,
        yes,
        force,
        ..
    } = &cli.cmd
    {
        if *live_mode && !*yes {
            bail!(
                "apply --live writes into {} and needs --yes (nothing was rendered)",
                home.display()
            );
        }
        // a credential value never reaches out/ or a live file, policy or no policy
        let leaks = sac::util::secret_paths(&desired);
        if !leaks.is_empty() {
            println!(
                "refusing to apply: {} secret-shaped value(s) in the desired state (keep references, never values)",
                leaks.len()
            );
            for l in leaks.iter().take(20) {
                println!("  {l}");
            }
            return Ok(1);
        }
        let v = validate(&desired);
        if !v.ok {
            println!("refusing to apply an invalid setup:");
            for e in &v.errors {
                println!("  {e}");
            }
            return Ok(1);
        }
        let pd = run_policies(&desired, &checks, &cctx);
        if !pd.ok {
            println!("refusing to apply: hard policy failures on desired state");
            println!("{}", paint_statuses(&format_policy_results(&pd, false)));
            return Ok(1);
        }
        let real_out = assert_safe_out_root(&out_root, &real_home())?;
        let r = render(
            &desired,
            &real_out,
            &home,
            Some(&format_policy_results(&pd, false)),
            Some(&format_plan(&p, false)),
        )?;
        println!(
            "apply: rendered {} file(s) into {} (nothing installed or loaded)",
            r.written.len(),
            rel(&r.out_dir, &cwd)
        );
        for f in &r.written {
            println!("  {}/{}", rel(&real_out, &cwd), f);
        }
        if *live_mode {
            if !*yes {
                bail!(
                    "apply --live writes into {} and needs --yes",
                    home.display()
                );
            }
            let lr = apply_live(
                &desired,
                &LiveOptions {
                    home: home.clone(),
                    yes: *yes,
                    force: *force,
                },
            )?;
            println!(
                "apply --live: {} file(s) changed in {}",
                lr.changed.len(),
                home.display()
            );
            for f in &lr.changed {
                println!("  wrote {f}");
            }
            for f in &lr.skipped {
                println!("  skipped {f}");
            }
            if let Some(b) = &lr.backup_dir {
                println!("  backups: {}", b.display());
            }
            if !lr.commands.is_empty() {
                println!("nothing was loaded. To activate the launchd jobs, run by hand:");
                for c in &lr.commands {
                    println!("  {c}");
                }
            }
        }
        return Ok(0);
    }

    bail!("unknown command");
}
