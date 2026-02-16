#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ChannelKind {
    Release,
    Preview,
}

impl ChannelKind {
    pub fn https_url(&self) -> &'static str {
        match self {
            ChannelKind::Release => "https://aka.ms/vs/17/release/channel",
            ChannelKind::Preview => "https://aka.ms/vs/17/pre/channel",
        }
    }

    pub fn vs_manifest_channel_id(&self) -> &'static str {
        match self {
            ChannelKind::Release => "Microsoft.VisualStudio.Manifests.VisualStudio",
            ChannelKind::Preview => "Microsoft.VisualStudio.Manifests.VisualStudioPreview",
        }
    }

    pub fn subdir(&self) -> &'static str {
        match self {
            ChannelKind::Release => "vs-release",
            ChannelKind::Preview => "vs-preview",
        }
    }

    pub fn channel_subdir(&self) -> &'static str {
        match self {
            ChannelKind::Release => "channel-release",
            ChannelKind::Preview => "channel-preview",
        }
    }

    pub fn channel_url_subdir(&self) -> &'static str {
        match self {
            ChannelKind::Release => "channel-release-url",
            ChannelKind::Preview => "channel-preview-url",
        }
    }
}
