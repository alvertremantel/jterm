pub use jones_editor::{ContentMode, EditorAction, EditorContext};

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn compatibility_reexports_editor_api() {
        let mut editor = EditorContext::from_content("hello");
        assert_eq!(ContentMode::Read.as_str(), "READ");
        assert!(matches!(
            editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            EditorAction::SaveFile
        ));
    }
}
