//! Tests for source-code language detection.

use super::*;

#[test]
fn common_extensions_map_to_stable_lowercase_names() {
    for (path, expected) in [
        ("main.rs", "rust"),
        ("app.py", "python"),
        ("index.js", "javascript"),
        ("view.jsx", "javascript"),
        ("lib.ts", "typescript"),
        ("component.tsx", "typescript"),
        ("server.go", "go"),
        ("Main.java", "java"),
        ("Main.kt", "kotlin"),
        ("App.swift", "swift"),
        ("lib.c", "c"),
        ("lib.h", "c"),
        ("lib.cpp", "cpp"),
        ("lib.hpp", "cpp"),
        ("lib.cc", "cpp"),
        ("Program.cs", "csharp"),
        ("app.rb", "ruby"),
        ("index.php", "php"),
        ("App.scala", "scala"),
        ("run.sh", "shell"),
        ("run.bash", "shell"),
        ("run.zsh", "shell"),
        ("query.sql", "sql"),
        ("init.lua", "lua"),
        ("model.r", "r"),
        ("View.m", "objective-c"),
        ("main.dart", "dart"),
        ("app.ex", "elixir"),
        ("test.exs", "elixir"),
        ("server.erl", "erlang"),
        ("Main.hs", "haskell"),
        ("main.ml", "ocaml"),
        ("core.clj", "clojure"),
        ("Cargo.toml", "toml"),
        ("ci.yaml", "yaml"),
        ("ci.yml", "yaml"),
        ("package.json", "json"),
        ("pom.xml", "xml"),
        ("site.css", "css"),
        ("site.scss", "scss"),
        ("App.vue", "vue"),
        ("App.svelte", "svelte"),
        ("main.zig", "zig"),
        ("main.nim", "nim"),
        ("api.proto", "protobuf"),
        ("schema.graphql", "graphql"),
    ] {
        assert_eq!(language_for_path(path), Some(expected), "{path}");
    }
}

#[test]
fn well_known_file_names_are_recognised_without_an_extension() {
    assert_eq!(language_for_path("Dockerfile"), Some("dockerfile"));
    assert_eq!(
        language_for_path("deploy/Dockerfile.dev"),
        Some("dockerfile")
    );
    assert_eq!(language_for_path("Makefile"), Some("makefile"));
    assert_eq!(language_for_path("GNUmakefile"), Some("makefile"));
    assert_eq!(language_for_path("CMakeLists.txt"), Some("cmake"));
}

#[test]
fn only_the_final_path_component_is_examined() {
    assert_eq!(language_for_path("src.rs/notes.md"), None);
    assert_eq!(language_for_path("a/b/c/main.rs"), Some("rust"));
    assert_eq!(language_for_path(r"C:\repo\main.py"), Some("python"));
}

#[test]
fn extension_matching_ignores_case() {
    assert_eq!(language_for_path("MAIN.RS"), Some("rust"));
    assert_eq!(language_for_path("Script.Py"), Some("python"));
}

#[test]
fn documents_are_not_code() {
    for path in [
        "readme.md",
        "notes.txt",
        "index.html",
        "page.htm",
        "doc.pdf",
    ] {
        assert_eq!(language_for_path(path), None, "{path}");
    }
}

#[test]
fn names_without_a_known_extension_are_not_code() {
    for path in [
        "",
        "README",
        ".bashrc",
        "archive.tar.gz",
        "photo.png",
        "dir/",
    ] {
        assert_eq!(language_for_path(path), None, "{path:?}");
    }
}
