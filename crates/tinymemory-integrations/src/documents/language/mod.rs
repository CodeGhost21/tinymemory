//! Source-code language detection from a file path.
//!
//! [`language_for_path`] answers "is this file code, and in which language"
//! from the file name alone: a handful of well-known names (`Dockerfile`,
//! `Makefile`, `CMakeLists.txt`) first, then the extension. The names it
//! returns are stable lowercase identifiers (`rust`, `python`, `typescript`)
//! that end up in [`tinymemory_api::MemoryMeta::language`], so they are a wire
//! contract: do not rename one.
//!
//! Markdown, plain text and HTML are documents, not code, and map to `None`;
//! [`crate::documents::DocumentFormat`] has its own variants for them.

/// Files recognised by their whole name rather than an extension.
///
/// Compared case-sensitively against the final path component, except that a
/// `Dockerfile.<suffix>` variant (`Dockerfile.dev`) still counts as a
/// Dockerfile.
const NAMED_FILES: &[(&str, &str)] = &[
    ("Dockerfile", "dockerfile"),
    ("Containerfile", "dockerfile"),
    ("Makefile", "makefile"),
    ("makefile", "makefile"),
    ("GNUmakefile", "makefile"),
    ("CMakeLists.txt", "cmake"),
    ("Rakefile", "ruby"),
    ("Gemfile", "ruby"),
    ("Justfile", "just"),
    ("justfile", "just"),
];

/// Extensions (lowercase, without the dot) and the language each names.
const EXTENSIONS: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("py", "python"),
    ("pyi", "python"),
    ("js", "javascript"),
    ("jsx", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("go", "go"),
    ("java", "java"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("swift", "swift"),
    ("c", "c"),
    ("h", "c"),
    ("cpp", "cpp"),
    ("cc", "cpp"),
    ("cxx", "cpp"),
    ("hpp", "cpp"),
    ("hh", "cpp"),
    ("hxx", "cpp"),
    ("cs", "csharp"),
    ("rb", "ruby"),
    ("php", "php"),
    ("scala", "scala"),
    ("sh", "shell"),
    ("bash", "shell"),
    ("zsh", "shell"),
    ("fish", "shell"),
    ("ps1", "powershell"),
    ("sql", "sql"),
    ("lua", "lua"),
    ("r", "r"),
    ("m", "objective-c"),
    ("mm", "objective-c"),
    ("dart", "dart"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("erl", "erlang"),
    ("hrl", "erlang"),
    ("hs", "haskell"),
    ("ml", "ocaml"),
    ("mli", "ocaml"),
    ("clj", "clojure"),
    ("cljs", "clojure"),
    ("edn", "clojure"),
    ("pl", "perl"),
    ("pm", "perl"),
    ("jl", "julia"),
    ("toml", "toml"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("json", "json"),
    ("jsonc", "json"),
    ("xml", "xml"),
    ("css", "css"),
    ("scss", "scss"),
    ("sass", "sass"),
    ("less", "less"),
    ("vue", "vue"),
    ("svelte", "svelte"),
    ("zig", "zig"),
    ("nim", "nim"),
    ("proto", "protobuf"),
    ("graphql", "graphql"),
    ("gql", "graphql"),
    ("tf", "terraform"),
    ("dockerfile", "dockerfile"),
    ("mk", "makefile"),
    ("cmake", "cmake"),
    ("gradle", "groovy"),
    ("groovy", "groovy"),
    ("sol", "solidity"),
];

/// The programming language a file holds, judged by its name.
///
/// `path` may be a bare file name or a path with either separator; only the
/// final component is examined. Returns `None` for documents (markdown, plain
/// text, HTML), for unknown extensions, and for names without one.
///
/// ```
/// use tinymemory_integrations::documents::language_for_path;
///
/// assert_eq!(language_for_path("src/main.rs"), Some("rust"));
/// assert_eq!(language_for_path("web/App.TSX"), Some("typescript"));
/// assert_eq!(language_for_path("docker/Dockerfile"), Some("dockerfile"));
/// assert_eq!(language_for_path("notes/readme.md"), None);
/// ```
#[must_use]
pub fn language_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if name.is_empty() {
        return None;
    }
    if let Some((_, language)) = NAMED_FILES.iter().find(|(known, _)| *known == name) {
        return Some(language);
    }
    if name.starts_with("Dockerfile.") || name.starts_with("Containerfile.") {
        return Some("dockerfile");
    }
    let (stem, extension) = name.rsplit_once('.')?;
    // A dotfile (`.bashrc`) has an empty stem; its "extension" is its name.
    if stem.is_empty() {
        return None;
    }
    let extension = extension.to_ascii_lowercase();
    EXTENSIONS
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, language)| *language)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
