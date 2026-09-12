//! Catalog profile owns both metadata dialect and generation wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderProfile {
    OpenAiChat,
    LmStudioChat,
    OpenAiResponses,
    LmStudioResponses,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationApi {
    ChatCompletions,
    Responses,
}

impl ProviderProfile {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "openai_compatible_chat" => Self::OpenAiChat,
            "lmstudio_chat" => Self::LmStudioChat,
            "openai_compatible_responses" => Self::OpenAiResponses,
            "lmstudio_responses" => Self::LmStudioResponses,
            _ => return None,
        })
    }
    pub(crate) fn native_metadata(self) -> bool {
        matches!(self, Self::LmStudioChat | Self::LmStudioResponses)
    }
    pub(crate) fn metadata_path(self) -> &'static str {
        if self.native_metadata() {
            "api/v1/models"
        } else {
            "v1/models"
        }
    }
    pub(crate) fn api(self) -> GenerationApi {
        match self {
            Self::OpenAiChat | Self::LmStudioChat => GenerationApi::ChatCompletions,
            Self::OpenAiResponses | Self::LmStudioResponses => GenerationApi::Responses,
        }
    }
}
impl GenerationApi {
    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat/completions",
            Self::Responses => "responses",
        }
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
        }
    }
    // Gateway exposes generation only, never LM Studio native metadata.
    pub(crate) fn gateway_profile(self) -> &'static str {
        match self {
            Self::ChatCompletions => "openai_compatible_chat",
            Self::Responses => "openai_compatible_responses",
        }
    }
}
