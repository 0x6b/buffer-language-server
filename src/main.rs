use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use serde_json::Value;
use tower_lsp::{jsonrpc::Result, lsp_types::*, Client, LanguageServer, LspService, Server};

const FAILED_TO_ACQUIRE_LOCK_MSG: &str = "failed to acquire lock";

fn get_char_index_from_position(s: &str, position: Position) -> usize {
    let line_start = s
        .lines()
        .take(position.line as usize)
        .map(|line| line.len() + 1)
        .sum::<usize>();

    let char_index = line_start + position.character as usize;

    if char_index > s.len() {
        s.len()
    } else {
        s.char_indices().nth(char_index).unwrap_or_default().0
    }
}

#[derive(Debug)]
struct Backend {
    client: Client,
    document_text: Arc<Mutex<String>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: Default::default(),
                    all_commit_characters: None,
                    ..Default::default()
                }),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client.log_message(MessageType::INFO, "initialized!").await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        *self.document_text.lock().expect(FAILED_TO_ACQUIRE_LOCK_MSG) = params.text_document.text;

        self.client.log_message(MessageType::INFO, "file opened!").await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        for change in params.content_changes {
            match change.range {
                Some(range) => {
                    let mut text = self.document_text.lock().expect(FAILED_TO_ACQUIRE_LOCK_MSG);

                    let start = get_char_index_from_position(text.as_str(), range.start);
                    let end = get_char_index_from_position(text.as_str(), range.end);

                    text.replace_range(start..end, change.text.as_str());
                }
                None => {
                    *self.document_text.lock().expect(FAILED_TO_ACQUIRE_LOCK_MSG) = change.text;
                }
            }
        }

        self.client.log_message(MessageType::INFO, "file changed!").await;
    }

    async fn did_save(&self, _: DidSaveTextDocumentParams) {
        self.client.log_message(MessageType::INFO, "file saved!").await;
    }

    async fn did_close(&self, _: DidCloseTextDocumentParams) {
        self.client.log_message(MessageType::INFO, "file closed!").await;
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let text = self.document_text.lock().expect("failed to acquire lock");
        let words = split(&text);
        let current_word = find_word_before_cursor(&text, params.text_document_position.position);

        Ok(Some(CompletionResponse::Array(
            HashSet::<&str>::from_iter(words)
                .into_iter()
                .filter_map(|word| {
                    if word == current_word {
                        return None;
                    }

                    Some(CompletionItem {
                        label: word.to_string(),
                        detail: None,
                        kind: Some(CompletionItemKind::TEXT),
                        ..CompletionItem::default()
                    })
                })
                .collect(),
        )))
    }

    async fn did_change_configuration(&self, _: DidChangeConfigurationParams) {
        self.client
            .log_message(MessageType::INFO, "configuration changed!")
            .await;
    }

    async fn did_change_workspace_folders(&self, _: DidChangeWorkspaceFoldersParams) {
        self.client
            .log_message(MessageType::INFO, "workspace folders changed!")
            .await;
    }

    async fn did_change_watched_files(&self, _: DidChangeWatchedFilesParams) {
        self.client
            .log_message(MessageType::INFO, "watched files have changed!")
            .await;
    }

    async fn execute_command(&self, _: ExecuteCommandParams) -> Result<Option<Value>> {
        self.client.log_message(MessageType::INFO, "command executed!").await;

        Ok(None)
    }
}

#[tokio::main]
async fn main() {
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());

    let (service, socket) = LspService::new(|client| Backend {
        client,
        document_text: Arc::new(Mutex::new(String::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[derive(Debug, Eq, PartialEq)]
enum CharCategory {
    Whitespace,
    Eol,
    Word,
    Punctuation,
    Unknown,
    Hiragana,
    Katakana,
    Kanji,
}

/// find a word at the given position, in the given text at current line
fn find_word_before_cursor(text: &str, position: Position) -> String {
    // From the start of the line to the cursor position, reversed
    let text_start_to_cursor = {
        let current_line = text.lines().nth(position.line as usize).unwrap_or_default();
        let byte_offset = current_line
            .char_indices()
            .nth(position.character as usize)
            .unwrap_or_default()
            .0;
        current_line.split_at(byte_offset).0.chars().rev().collect::<String>()
    };

    let mut word = String::new();

    for i in 0..=text_start_to_cursor.chars().count() {
        if let Some(ch) = text_start_to_cursor.chars().nth(i) {
            word.push(ch);
            if is_boundary(ch, text_start_to_cursor.chars().nth(i + 1).unwrap_or(' ')) {
                break;
            }
        }
    }

    word
}

fn split(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut word_start = 0;
    let mut last_category = categorize_char(s.chars().next().unwrap_or_default());

    for (i, ch) in s.char_indices() {
        let current_category = categorize_char(ch);
        if current_category != last_category {
            result.push(&s[word_start..i]);
            word_start = i;
            last_category = current_category;
        }
    }

    if word_start < s.len() {
        result.push(&s[word_start..]);
    }

    result
}

fn is_boundary(a: char, b: char) -> bool {
    categorize_char(a) != categorize_char(b)
}

#[inline]
fn categorize_char(ch: char) -> CharCategory {
    if char_is_hiragana(ch) {
        CharCategory::Hiragana
    } else if char_is_katakana(ch) {
        CharCategory::Katakana
    } else if char_is_kanji(ch) {
        CharCategory::Kanji
    } else if char_is_line_ending(ch) {
        CharCategory::Eol
    } else if char_is_whitespace(ch) {
        CharCategory::Whitespace
    } else if char_is_word(ch) {
        CharCategory::Word
    } else if char_is_punctuation(ch) {
        CharCategory::Punctuation
    } else {
        CharCategory::Unknown
    }
}

// Determine whether a character is a hiragana character.
#[inline]
fn char_is_hiragana(ch: char) -> bool {
    ('\u{3041}'..='\u{3096}').contains(&ch) || ('\u{3099}'..='\u{309F}').contains(&ch) // Hiragana: https://www.unicode.org/charts/PDF/U3040.pdf
        || ('\u{1B100}'..='\u{1B12F}').contains(&ch) // Kana Extended-A: https://www.unicode.org/charts/PDF/U1B100.pdf
        || ('\u{1AFF0}'..='\u{1AFFF}').contains(&ch) // Kana Extended-B: https://www.unicode.org/charts/PDF/U1AFF0.pdf
        || ('\u{1B000}'..='\u{1B0FF}').contains(&ch) // Kana Supplement: https://www.unicode.org/charts/PDF/U1B000.pdf
        || ('\u{1B130}'..='\u{1B16F}').contains(&ch) // Small Kana Extension: https://www.unicode.org/charts/PDF/U1B130.pdf
}

// Determine whether a character is a katakana character.
#[inline]
fn char_is_katakana(ch: char) -> bool {
    ('\u{30A0}'..='\u{30FF}').contains(&ch) // Katakana: https://www.unicode.org/charts/PDF/U30A0.pdf
}

// Determine whether a character is a kanji, or CJK Unified Ideographs, character.
#[inline]
fn char_is_kanji(ch: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&ch) // CJK Unified Ideographs: https://www.unicode.org/charts/PDF/U4E00.pdf
        || ('\u{3400}'..='\u{4DBF}').contains(&ch) // CJK Unified Ideographs Extension A: https://www.unicode.org/charts/PDF/U3400.pdf
        || ('\u{20000}'..='\u{2A6DF}').contains(&ch) // CJK Unified Ideographs Extension B: https://www.unicode.org/charts/PDF/U20000.pdf
        || ('\u{2A700}'..='\u{2B739}').contains(&ch) // CJK Unified Ideographs Extension C: https://www.unicode.org/charts/PDF/U2A700.pdf
        || ('\u{2B740}'..='\u{2B81D}').contains(&ch) // CJK Unified Ideographs Extension D: https://www.unicode.org/charts/PDF/U2B740.pdf
        || ('\u{2B820}'..='\u{2CEA1}').contains(&ch) // CJK Unified Ideographs Extension E: https://www.unicode.org/charts/PDF/U2B820.pdf
        || ('\u{2CEB0}'..='\u{2EBE0}').contains(&ch) // CJK Unified Ideographs Extension F: https://www.unicode.org/charts/PDF/U2CEB0.pdf
        || ('\u{30000}'..='\u{3134A}').contains(&ch) // CJK Unified Ideographs Extension G: https://www.unicode.org/charts/PDF/U30000.pdf
        || ('\u{31350}'..='\u{323AF}').contains(&ch) // CJK Unified Ideographs Extension H: https://www.unicode.org/charts/PDF/U31350.pdf
        || ('\u{2EBF0}'..='\u{2EE5D}').contains(&ch) // CJK Unified Ideographs Extension H: https://www.unicode.org/charts/PDF/U2EBF0.pdf
        || ('\u{F900}'..='\u{FAFF}').contains(&ch) // CJK Compatibility Ideographs: https://www.unicode.org/charts/PDF/UF900.pdf
        || ('\u{2F800}'..='\u{2FA1F}').contains(&ch) // CJK Compatibility Ideographs Supplement: https://www.unicode.org/charts/PDF/U2F800.pdf
}

// Determine whether a character is a line ending.
#[inline]
fn char_is_line_ending(ch: char) -> bool {
    matches!(
        ch,
        '\u{000A}' | '\u{000B}' | '\u{000C}' | '\u{000D}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

#[inline]
fn char_is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[inline]
fn char_is_punctuation(ch: char) -> bool {
    use unicode_general_category::{get_general_category, GeneralCategory};

    matches!(
        get_general_category(ch),
        GeneralCategory::OtherPunctuation
            | GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::ConnectorPunctuation
            | GeneralCategory::DashPunctuation
            | GeneralCategory::MathSymbol
            | GeneralCategory::CurrencySymbol
            | GeneralCategory::ModifierSymbol
    )
}

/// Determine whether a character qualifies as (non-line-break)
/// whitespace.
#[inline]
fn char_is_whitespace(ch: char) -> bool {
    // TODO: this is a naive binary categorization of whitespace
    // characters.  For display, word wrapping, etc. we'll need a better
    // categorization based on e.g. breaking vs non-breaking spaces
    // and whether they're zero-width or not.
    match ch {
        //'\u{1680}' | // Ogham Space Mark (here for completeness, but usually displayed as a dash, not as whitespace)
        '\u{0009}' | // Character Tabulation
        '\u{0020}' | // Space
        '\u{00A0}' | // No-break Space
        '\u{180E}' | // Mongolian Vowel Separator
        '\u{202F}' | // Narrow No-break Space
        '\u{205F}' | // Medium Mathematical Space
        '\u{3000}' | // Ideographic Space
        '\u{FEFF}'   // Zero Width No-break Space
        => true,

        // En Quad, Em Quad, En Space, Em Space, Three-per-em Space,
        // Four-per-em Space, Six-per-em Space, Figure Space,
        // Punctuation Space, Thin Space, Hair Space, Zero Width Space.
        ch if ('\u{2000}' ..= '\u{200B}').contains(&ch) => true,

        _ => false,
    }
}
