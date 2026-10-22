//! Lexical (BM25) recall over memory-v2 notes.
//!
//! Recall reads `topics/*.md` and `observations/_inbox/*.md` straight from
//! disk on every query, so notes the agent just wrote are searchable without
//! waiting for an index refresh. Stores are small and the reads are bounded
//! by [`MAX_FILES_PER_SCOPE`] and [`MAX_FILE_BYTES`].

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use supercharge_tools::types::memory_backend::{MemoryBackend, MemorySearchResult};

use crate::V2MemoryScope;
use crate::query_expansion::extract_keywords;

const MAX_FILES_PER_SCOPE: usize = 512;
const MAX_FILE_BYTES: u64 = 64 * 1024;
const TARGET_CHUNK_CHARS: usize = 700;
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;
const MAX_GET_LINES: usize = 400;

/// One recalled note fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct V2RecallHit {
    /// Stable id: `<absolute path>:<start line>`.
    pub chunk_id: String,
    pub path: PathBuf,
    pub scope: V2MemoryScope,
    /// 0-based start line.
    pub start_line: usize,
    /// 0-based exclusive end line.
    pub end_line: usize,
    /// Normalized relevance in `[0, 1)`.
    pub score: f64,
    pub text: String,
}

struct Chunk {
    path: PathBuf,
    scope: V2MemoryScope,
    start_line: usize,
    end_line: usize,
    text: String,
    terms: HashMap<String, u32>,
    len: usize,
}

fn tokenize(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| word.len() >= 2)
        .map(str::to_lowercase)
}

fn list_markdown(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    files.truncate(MAX_FILES_PER_SCOPE);
    files
}

/// Split a note into paragraph-aligned chunks of roughly
/// [`TARGET_CHUNK_CHARS`]. YAML front matter is skipped.
fn chunk_file(path: &Path, scope: V2MemoryScope, content: &str, out: &mut Vec<Chunk>) {
    let lines: Vec<&str> = content.lines().collect();
    let mut index = 0;
    if lines.first().is_some_and(|line| line.trim() == "---")
        && let Some(close) = lines.iter().skip(1).position(|line| line.trim() == "---")
    {
        index = close + 2;
    }
    let mut start = index;
    let mut buffer = String::new();
    let flush = |start: usize, end: usize, buffer: &mut String, out: &mut Vec<Chunk>| {
        let text = buffer.trim();
        if !text.is_empty() {
            let mut terms = HashMap::new();
            let mut len = 0;
            for token in tokenize(text) {
                *terms.entry(token).or_insert(0) += 1;
                len += 1;
            }
            if len > 0 {
                out.push(Chunk {
                    path: path.to_path_buf(),
                    scope,
                    start_line: start,
                    end_line: end,
                    text: text.to_string(),
                    terms,
                    len,
                });
            }
        }
        buffer.clear();
    };
    while index < lines.len() {
        let line = lines[index];
        let boundary = line.trim().is_empty() || line.starts_with('#');
        if boundary && buffer.len() >= TARGET_CHUNK_CHARS {
            flush(start, index, &mut buffer, out);
            start = index;
        }
        if buffer.is_empty() && line.trim().is_empty() {
            start = index + 1;
        } else {
            buffer.push_str(line);
            buffer.push('\n');
        }
        index += 1;
    }
    flush(start, lines.len(), &mut buffer, out);
}

fn load_scope(scope_dir: &Path, scope: V2MemoryScope, out: &mut Vec<Chunk>) {
    let excluded: BTreeSet<String> =
        crate::v2::excluded_manifest_paths(scope_dir).unwrap_or_default();
    for relative_dir in ["topics", "observations/_inbox"] {
        for path in list_markdown(&scope_dir.join(relative_dir)) {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if excluded.contains(&format!("{relative_dir}/{name}")) {
                continue;
            }
            if std::fs::metadata(&path).map_or(true, |meta| meta.len() > MAX_FILE_BYTES) {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                chunk_file(&path, scope, &content, out);
            }
        }
    }
}

/// Rank note chunks in the given scopes against `query`.
///
/// The score blends keyword coverage with a squashed BM25 value so a single
/// incidental word match stays below typical thresholds.
pub fn search_v2_notes(
    scopes: &[(&Path, V2MemoryScope)],
    query: &str,
    max_results: usize,
    min_score: f64,
) -> Vec<V2RecallHit> {
    let keywords = extract_keywords(query);
    if keywords.is_empty() || max_results == 0 {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    for (dir, scope) in scopes {
        load_scope(dir, *scope, &mut chunks);
    }
    if chunks.is_empty() {
        return Vec::new();
    }
    let total = chunks.len() as f64;
    let average_len = chunks.iter().map(|chunk| chunk.len).sum::<usize>() as f64 / total;
    let idf: HashMap<&str, f64> = keywords
        .iter()
        .map(|keyword| {
            let frequency = chunks
                .iter()
                .filter(|chunk| chunk.terms.contains_key(keyword))
                .count() as f64;
            let value = ((total - frequency + 0.5) / (frequency + 0.5) + 1.0).ln();
            (keyword.as_str(), value)
        })
        .collect();
    let mut hits: Vec<V2RecallHit> = chunks
        .into_iter()
        .filter_map(|chunk| {
            let mut matched = 0usize;
            let mut bm25 = 0.0;
            for keyword in &keywords {
                let Some(&count) = chunk.terms.get(keyword) else {
                    continue;
                };
                matched += 1;
                let tf = f64::from(count);
                let norm = BM25_K1 * (1.0 - BM25_B + BM25_B * chunk.len as f64 / average_len);
                bm25 += idf[keyword.as_str()] * tf * (BM25_K1 + 1.0) / (tf + norm);
            }
            if matched == 0 {
                return None;
            }
            let coverage = matched as f64 / keywords.len() as f64;
            let score = 0.5 * coverage + 0.5 * (bm25 / (bm25 + 2.0));
            (score >= min_score).then(|| V2RecallHit {
                chunk_id: format!("{}:{}", chunk.path.display(), chunk.start_line),
                path: chunk.path,
                scope: chunk.scope,
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                score,
                text: chunk.text,
            })
        })
        .collect();
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    hits.truncate(max_results);
    hits
}

fn scope_label(scope: V2MemoryScope) -> &'static str {
    match scope {
        V2MemoryScope::Global => "global",
        V2MemoryScope::Workspace => "workspace",
    }
}

/// `memory_search` / `memory_get` over memory-v2 notes.
#[derive(Debug, Clone)]
pub struct V2MemoryBackend {
    global_dir: PathBuf,
    workspace_dir: PathBuf,
    default_min_score: f64,
}

impl V2MemoryBackend {
    pub fn new(global_dir: &Path, workspace_dir: &Path, default_min_score: f64) -> Self {
        Self {
            global_dir: global_dir.to_path_buf(),
            workspace_dir: workspace_dir.to_path_buf(),
            default_min_score,
        }
    }

    pub fn search_hits(&self, query: &str, max_results: usize, min_score: f64) -> Vec<V2RecallHit> {
        search_v2_notes(
            &[
                (&self.workspace_dir, V2MemoryScope::Workspace),
                (&self.global_dir, V2MemoryScope::Global),
            ],
            query,
            max_results,
            min_score,
        )
    }

    /// Resolve `path` to a readable note inside one of the scopes. Relative
    /// paths resolve against the workspace scope first, then global.
    fn resolve_note(&self, path: &str) -> Option<PathBuf> {
        let requested = Path::new(path.trim());
        let candidates: Vec<PathBuf> = if requested.is_absolute() {
            vec![requested.to_path_buf()]
        } else {
            vec![
                self.workspace_dir.join(requested),
                self.global_dir.join(requested),
            ]
        };
        let roots = [
            self.workspace_dir.canonicalize().ok()?,
            self.global_dir.canonicalize().ok()?,
        ];
        candidates.into_iter().find_map(|candidate| {
            let canonical = candidate.canonicalize().ok()?;
            let root = roots.iter().find(|root| canonical.starts_with(root))?;
            let relative = canonical.strip_prefix(root).ok()?;
            let readable = relative == Path::new("MEMORY.md")
                || (canonical.extension().is_some_and(|ext| ext == "md")
                    && (relative.parent() == Some(Path::new("topics"))
                        || relative.parent() == Some(Path::new("observations/_inbox"))));
            (readable && canonical.is_file()).then_some(canonical)
        })
    }
}

#[async_trait::async_trait]
impl MemoryBackend for V2MemoryBackend {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        min_score: f64,
    ) -> Result<Vec<MemorySearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        let backend = self.clone();
        let query = query.to_string();
        let hits = tokio::task::spawn_blocking(move || {
            backend.search_hits(&query, max_results, min_score)
        })
        .await?;
        Ok(hits
            .into_iter()
            .map(|hit| MemorySearchResult {
                chunk_id: hit.chunk_id,
                path: hit.path.display().to_string(),
                start_line: hit.start_line,
                end_line: hit.end_line,
                score: hit.score,
                snippet: hit.text,
                source: scope_label(hit.scope).to_string(),
                created_at: None,
            })
            .collect())
    }

    fn get(
        &self,
        path: &str,
        from: Option<usize>,
        lines: Option<usize>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let resolved = self
            .resolve_note(path)
            .ok_or_else(|| format!("not a readable memory note: {path}"))?;
        if std::fs::metadata(&resolved)?.len() > MAX_FILE_BYTES * 4 {
            return Err(format!("memory note is too large to read: {path}").into());
        }
        let content = std::fs::read_to_string(&resolved)?;
        let start = from.unwrap_or(1).saturating_sub(1);
        let count = lines.unwrap_or(MAX_GET_LINES).min(MAX_GET_LINES);
        Ok(content
            .lines()
            .skip(start)
            .take(count)
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn total_chunks(&self) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let mut chunks = Vec::new();
        load_scope(&self.workspace_dir, V2MemoryScope::Workspace, &mut chunks);
        load_scope(&self.global_dir, V2MemoryScope::Global, &mut chunks);
        Ok(chunks.len())
    }

    fn default_search_max_results(&self) -> usize {
        6
    }

    fn default_search_min_score(&self) -> f64 {
        self.default_min_score
    }
}

#[cfg(test)]
#[path = "v2_recall_tests.rs"]
mod tests;
