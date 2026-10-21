use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn feature_needs_are_the_scripts_need_lines() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../images/base/root/usr/local/lib/wadspaces/features");
    let mut found: Vec<(String, String)> = Vec::new();
    for e in std::fs::read_dir(&dir).expect("the feature scripts") {
        let path = e.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            if let Some(n) = line.trim().strip_prefix("need ") {
                found.push((name.clone(), n.trim().to_string()));
            }
        }
    }
    found.sort();
    let mut table: Vec<(String, String)> =
        FEATURE_NEEDS.iter().flat_map(|(f, ns)| ns.iter().map(move |n| (f.to_string(), n.to_string()))).collect();
    table.sort();
    assert_eq!(table, found, "spec::FEATURE_NEEDS and the feature scripts' `need` lines differ");
}

#[test]
fn package_ids() {
    assert_eq!(of_app("vscode").as_deref(), Some("vscode"));
    assert_eq!(of_app("lmms").as_deref(), Some("apt-lmms"));
    assert_eq!(of_app("wadbrowser"), None, "the base has it");
    assert_eq!(of_app("nope"), None);
    assert_eq!(of_tool("rust").as_deref(), Some("rust"));
    assert_eq!(of_tool("rm -rf"), None);
    assert_eq!(of_extension("Rust-Lang.rust-analyzer").as_deref(), Some("ext-rust-lang.rust-analyzer"));
    assert_eq!(of_extension("bad id; rm"), None);
    assert_eq!(source("vscode"), Some(Source::Feature("vscode")));
    assert!(matches!(source("apt-lmms"), Some(Source::Apt { ref app, .. }) if app == "lmms"));
    assert_eq!(source("apt-vscode"), None, "vscode isn't a Debian app");
    assert_eq!(source("ext-vadimcn.vscode-lldb"), Some(Source::Extension("vadimcn.vscode-lldb".into())));
    assert_eq!(source("whatever"), None);
}

#[test]
fn closure_adds_what_packages_need_in_order() {
    assert_eq!(
        closure(&["ext-rust-lang.rust-analyzer", "rust", "tiled", "apt-lmms"]),
        s(&["cpp", "rust", "electron-deps", "vscode", "tiled", "apt-lmms", "ext-rust-lang.rust-analyzer"])
    );
    assert_eq!(closure(&["claude-code"]), s(&["nodejs", "claude-code"]));
    assert_eq!(closure::<&str>(&[]), Vec::<String>::new());
}

#[test]
fn stack_order_builds_on_what_the_machine_stores() {
    let iq = s(&["cpp", "electron-deps", "vscode", "tiled", "libresprite", "ext-ms-vscode.cpptools"]);
    // Rust added: IQ's whole chain, then the new ones.
    let mut want = iq.clone();
    want.extend(s(&["rust", "ext-rust-lang.rust-analyzer"]));
    assert_eq!(
        stack_order(&closure(&want), std::slice::from_ref(&iq)),
        s(&[
            "cpp",
            "electron-deps",
            "vscode",
            "tiled",
            "libresprite",
            "ext-ms-vscode.cpptools",
            "rust",
            "ext-rust-lang.rust-analyzer"
        ])
    );
    // tiled taken out: the chain up to it, then the rest.
    let want = closure(&["cpp", "vscode", "libresprite", "ext-ms-vscode.cpptools"]);
    assert_eq!(
        stack_order(&want, std::slice::from_ref(&iq)),
        s(&["cpp", "electron-deps", "vscode", "libresprite", "ext-ms-vscode.cpptools"])
    );
    // Nothing stored: WadSpaces' order.
    assert_eq!(stack_order(&want, &[]), want);
    // The longest of several.
    let short = s(&["cpp"]);
    assert_eq!(
        stack_order(&s(&["cpp", "tiled", "vscode", "electron-deps"]), &[short, iq])[..3],
        s(&["cpp", "electron-deps", "vscode"])
    );
}

#[test]
fn a_package_containerfile_captures_on_its_needs() {
    let f = package_containerfile(
        "ext-rust-lang.rust-analyzer",
        "localhost/wadspaces-base:trixie",
        &[
            ("electron-deps".into(), "localhost/wadspaces-pkg-electron-deps:e1ab031adc3d".into()),
            ("vscode".into(), "localhost/wadspaces-pkg-vscode:52e5f2a601a8".into()),
        ],
        true,
    )
    .unwrap();
    assert!(
        f.starts_with(
            "# The ext-rust-lang.rust-analyzer package: what the VS Code extension rust-lang.rust-analyzer adds"
        ),
        "{f}"
    );
    assert!(f.contains("ARG BASE_IMAGE=localhost/wadspaces-base:trixie\nFROM ${BASE_IMAGE} AS build\n"), "{f}");
    assert!(f.contains("COPY --from=localhost/wadspaces-pkg-electron-deps:e1ab031adc3d / /\nCOPY --from=localhost/wadspaces-pkg-vscode:52e5f2a601a8 / /\nRUN wadspaces-pkg assemble\n"), "{f}");
    assert!(f.contains(r#"RUN --mount=type=secret,id=github_token ["wadspaces-pkg","capture","ext-rust-lang.rust-analyzer","--out","/out","--needs","electron-deps,vscode","--probe","#), "{f}");
    assert!(f.contains(r#""--","wadspaces-vscode-extensions","rust-lang.rust-analyzer"]"#), "{f}");
    assert!(f.ends_with("FROM scratch\nCOPY --from=build /out/ /\nLABEL io.wadspaces.pkg=\"ext-rust-lang.rust-analyzer\" io.wadspaces.pkg.needs=\"electron-deps,vscode\"\n"), "{f}");

    let apt = package_containerfile("apt-lmms", "b", &[], false).unwrap();
    assert!(apt.contains(r#"RUN ["wadspaces-pkg","capture","apt-lmms","--out","/out","--probe","dpkg-query -W -f='${Version}' lmms","--","sh","-c","wadspaces-apt lmms --desktop lmms.desktop lmms && wadspaces-apt --clean"]"#), "{apt}");
    assert!(!apt.contains("assemble"), "no needs, nothing to assemble: {apt}");

    assert_eq!(
        package_containerfile("ext-a.b", "b", &[("vscode".into(), "v".into())], false),
        Err("ext-a.b needs electron-deps first".into())
    );
    assert!(package_containerfile("nope", "b", &[], false).is_err());
}

#[test]
fn chains_and_their_labels() {
    let pkgs = vec![
        ("cpp".to_string(), local_ref("cpp", "sha256:41fc6e02a4d9aaaa")),
        ("rust".to_string(), local_ref("rust", "448760a5be45bbbb")),
    ];
    assert_eq!(pkgs[0].1, "localhost/wadspaces-pkg-cpp:41fc6e02a4d9");
    // The same packages on the same base: the same chain.
    let c = chain_ref("sha256:base1", &pkgs);
    assert!(c.starts_with("localhost/wadspaces-chain:") && c.len() == CHAIN.len() + 13, "{c}");
    assert_eq!(c, chain_ref("base1", &pkgs));
    assert_ne!(c, chain_ref("base2", &pkgs), "another base");
    assert_ne!(c, chain_ref("base1", &pkgs[..1]), "a shorter chain");
    let reversed: Vec<_> = pkgs.iter().rev().cloned().collect();
    assert_ne!(c, chain_ref("base1", &reversed), "another order");
    assert_eq!(
        chain_step("localhost/wadspaces-chain:aaaaaaaaaaaa", &pkgs[1].1, &pkgs),
        "# A chain image. Generated by WadSpaces.\nFROM localhost/wadspaces-chain:aaaaaaaaaaaa\n\
         COPY --from=localhost/wadspaces-pkg-rust:448760a5be45 / /\n\
         LABEL io.wadspaces.chain=\"cpp@41fc6e02a4d9,rust@448760a5be45\"\n"
    );
    assert_eq!(
        assembled(&c, true).join("\n"),
        format!("ARG BASE_IMAGE={c}\nFROM ${{BASE_IMAGE}}\n\nRUN wadspaces-pkg assemble")
    );
    assert!(!assembled("localhost/wadspaces-base:trixie", false).join("\n").contains("assemble"));
    assert_eq!(pkgs_label(&pkgs), "cpp@41fc6e02a4d9,rust@448760a5be45");
    assert_eq!(pkgs_of_label(&pkgs_label(&pkgs)), s(&["cpp", "rust"]));
    assert_eq!(pkgs_of_label(""), Vec::<String>::new());
    assert_eq!(remote_ref("vscode", "trixie.1"), "ghcr.io/hunggod/wadspaces-pkg-vscode:trixie.1");
}

#[test]
fn ci_builds_the_catalog_in_waves() {
    let all = all();
    for id in ["electron-deps", "vscode", "nodejs", "apt-lmms", "ext-anthropic.claude-code", "rust", "cpp"] {
        assert!(all.iter().any(|a| a == id), "{id} missing from {all:?}");
    }
    assert!(!all.iter().any(|a| a == "wadbrowser"));
    let waves = waves(&all);
    let wave_of = |id: &str| waves.iter().position(|w| w.iter().any(|x| x == id)).unwrap();
    assert_eq!(wave_of("electron-deps"), 0);
    assert_eq!(wave_of("vscode"), 1);
    assert_eq!(wave_of("ext-anthropic.claude-code"), 2);
    assert!(wave_of("rust") > wave_of("cpp"));
    assert_eq!(waves.iter().map(Vec::len).sum::<usize>(), all.len());
}

#[test]
fn the_index_keeps_other_abis_and_takes_new_packages() {
    let old = serde_json::json!({
        "version": 1, "registry": REGISTRY,
        "abis": {
            "trixie.0": { "base": "sha256:old", "packages": { "cpp": { "version": "3.30" } } },
            "trixie.1": { "base": "sha256:b1", "packages": { "cpp": { "version": "3.31" }, "rust": { "version": "1.98" } } }
        }
    });
    let results = [
        serde_json::json!({ "id": "rust", "digest": "sha256:d", "imageId": "sha256:i", "version": "1.99.1", "size": 1461000000u64 }),
        serde_json::json!({ "id": "not a package", "version": "x" }),
    ];
    let new = merge_index(&old, "trixie.1", "sha256:b2", &results);
    assert_eq!(new["abis"]["trixie.0"], old["abis"]["trixie.0"]);
    assert_eq!(new["abis"]["trixie.1"]["base"], "sha256:b2");
    assert_eq!(in_index(&new, "trixie.1", "cpp").unwrap()["version"], "3.31");
    assert_eq!(in_index(&new, "trixie.1", "rust").unwrap()["version"], "1.99.1");
    assert!(in_index(&new, "trixie.1", "rust").unwrap().get("id").is_none());
    assert!(in_index(&new, "trixie.1", "not a package").is_none());
    let fresh = merge_index(&serde_json::Value::Null, "trixie.1", "sha256:b", &results[..1]);
    assert_eq!(fresh["version"], 1);
    assert_eq!(fresh["registry"], REGISTRY);
    assert!(in_index(&fresh, "trixie.1", "rust").is_some());
    assert!(in_index(&fresh, "trixie.2", "rust").is_none());
}

#[test]
fn packages_are_called_by_their_apps_tools_and_extensions_names() {
    assert_eq!(name("vscode"), "VS Code");
    assert_eq!(name("rust"), "Rust");
    assert_eq!(name("ext-rust-lang.rust-analyzer"), "rust-analyzer");
    assert_eq!(name("ext-someone.unknown"), "someone.unknown");
    assert_eq!(name("electron-deps"), "Electron's libraries");
    assert_eq!(name("nothing-known"), "nothing-known");
}
