//! The `WordCounter` external component of the `externals` test program.

use interface::{Length, Word, WordCounter};

struct Counter {
    count: u64,
}

impl WordCounter for Counter {
    fn new() -> Self {
        Counter { count: 0 }
    }

    fn add(&mut self, text: &str) -> u64 {
        for word in text.split_whitespace() {
            assert_ne!(word, "panic", "asked to panic");
            self.count += 1;
        }
        self.count
    }

    fn split(&self, text: String) -> Vec<Word> {
        let mut words = Vec::new();
        let mut after = None;
        for word in text.split_whitespace() {
            let letters = word.chars().count() as u32;
            let length = if letters <= 3 {
                Length::Short
            } else {
                Length::Long(letters)
            };
            words.push(Word {
                text: word.to_string(),
                length,
                after: after.clone(),
            });
            after = Some(word.to_string());
        }
        words
    }

    fn read_file(&self, path: &str) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|e| e.to_string())
    }
}

interface::export!(Counter);
