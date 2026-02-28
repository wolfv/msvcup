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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_urls_are_https() {
        assert!(ChannelKind::Release.https_url().starts_with("https://"));
        assert!(ChannelKind::Preview.https_url().starts_with("https://"));
    }

    #[test]
    fn release_and_preview_have_different_urls() {
        assert_ne!(
            ChannelKind::Release.https_url(),
            ChannelKind::Preview.https_url()
        );
    }

    #[test]
    fn manifest_channel_ids_differ() {
        assert_ne!(
            ChannelKind::Release.vs_manifest_channel_id(),
            ChannelKind::Preview.vs_manifest_channel_id()
        );
    }

    #[test]
    fn subdirs_are_distinct() {
        assert_ne!(ChannelKind::Release.subdir(), ChannelKind::Preview.subdir());
        assert_ne!(
            ChannelKind::Release.channel_subdir(),
            ChannelKind::Preview.channel_subdir()
        );
        assert_ne!(
            ChannelKind::Release.channel_url_subdir(),
            ChannelKind::Preview.channel_url_subdir()
        );
    }

    #[test]
    fn subdirs_contain_release_or_preview() {
        assert!(ChannelKind::Release.subdir().contains("release"));
        assert!(ChannelKind::Preview.subdir().contains("preview"));
    }
}
