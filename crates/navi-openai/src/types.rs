#[derive(Debug, Clone, Copy)]
pub enum OpenAiApiKind {
    Responses,
    ChatCompletions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamRoute {
    Responses,
    ChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
}

