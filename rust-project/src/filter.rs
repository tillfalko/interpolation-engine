pub struct OutputFilter {
    start_str: String,
    stop_str: String,
    enumerate_outputs: bool,
    buffer: String,
    shown: bool,
    outputs: Vec<String>,
}

impl OutputFilter {
    pub fn new(start_str: &str, stop_str: &str, enumerate_outputs: bool) -> Self {
        Self {
            start_str: start_str.to_string(),
            stop_str: stop_str.to_string(),
            enumerate_outputs,
            buffer: String::new(),
            shown: false,
            outputs: Vec::new(),
        }
    }

    pub fn update(&mut self, chunk: &str) -> String {
        if self.start_str.is_empty() || self.stop_str.is_empty() {
            if self.outputs.is_empty() {
                self.outputs.push(String::new());
            }
            self.outputs.last_mut().unwrap().push_str(chunk);
            return chunk.to_string();
        }

        self.buffer.push_str(chunk);
        let next_str = if self.shown { &self.stop_str } else { &self.start_str };
        let mut enumeration = String::new();
        if self.buffer.starts_with(next_str) && !next_str.is_empty() {
            self.buffer = self.buffer[next_str.len()..].to_string();
            self.shown = !self.shown;
            if self.shown {
                self.outputs.push(String::new());
                if self.enumerate_outputs {
                    if self.outputs.len() > 1 {
                        enumeration.push_str("\n\n");
                    }
                    enumeration.push_str(&format!("{}. ", self.outputs.len()));
                }
            }
        }

        let safe = safe_index(&self.buffer, next_str);

        let delta = if self.shown {
            self.buffer[..safe].to_string()
        } else {
            String::new()
        };
        self.buffer = self.buffer[safe..].to_string();
        if self.shown && !self.outputs.is_empty() {
            self.outputs.last_mut().unwrap().push_str(&delta);
        }
        format!("{enumeration}{delta}")
    }

    pub fn outputs(&self) -> Vec<String> {
        self.outputs.clone()
    }
}

pub struct InvertedFilter {
    start_str: String,
    stop_str: String,
    buffer: String,
    shown: bool,
}

impl InvertedFilter {
    pub fn new(start_str: &str, stop_str: &str) -> Self {
        Self {
            start_str: start_str.to_string(),
            stop_str: stop_str.to_string(),
            buffer: String::new(),
            shown: true,
        }
    }

    pub fn update(&mut self, chunk: &str) -> String {
        self.buffer.push_str(chunk);
        let next_str = if self.shown { &self.start_str } else { &self.stop_str };

        if self.buffer.starts_with(next_str) && !next_str.is_empty() {
            self.buffer = self.buffer[next_str.len()..].to_string();
            self.shown = !self.shown;
        }

        let safe = safe_index(&self.buffer, next_str);
        let delta = if self.shown {
            self.buffer[..safe].to_string()
        } else {
            String::new()
        };
        self.buffer = self.buffer[safe..].to_string();
        delta
    }
}

fn safe_index(buffer: &str, next_str: &str) -> usize {
    if next_str.is_empty() {
        return buffer.len();
    }
    let mut safe = buffer.len();
    for (i, _) in buffer.char_indices() {
        if next_str.starts_with(&buffer[i..]) {
            safe = i;
            break;
        }
    }
    safe
}
