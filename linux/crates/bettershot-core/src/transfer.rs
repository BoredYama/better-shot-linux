//! Export and share progress, the model behind `Sources/Views/TransferStatusCard.swift`.
//! The capture deck shows it compactly; the editor shows the full card.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Processing,
    Rendering,
    Uploading,
    Exporting,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Processing => "Processing",
            Self::Rendering => "Rendering",
            Self::Uploading => "Uploading",
            Self::Exporting => "Exporting",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferStatus {
    /// `progress` in 0...1 when known.
    Working {
        stage: Stage,
        progress: Option<f64>,
    },
    LinkReady {
        url: String,
    },
    Exported {
        path: std::path::PathBuf,
    },
    Failed {
        headline: String,
        message: String,
        can_retry: bool,
    },
}

impl TransferStatus {
    pub fn failed(headline: &str, message: &str) -> Self {
        Self::Failed { headline: headline.into(), message: message.into(), can_retry: true }
    }

    pub fn is_working(&self) -> bool {
        matches!(self, Self::Working { .. })
    }

    /// The compact card's title.
    pub fn compact_title(&self) -> &str {
        match self {
            Self::Working { stage, .. } => stage.label(),
            Self::LinkReady { .. } => "Link copied",
            Self::Failed { .. } => "Share failed",
            Self::Exported { .. } => "Saved",
        }
    }

    /// The compact card's detail line, for working states.
    pub fn progress_text(&self) -> Option<String> {
        match self {
            Self::Working { progress: Some(p), .. } => {
                let p = if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 };
                Some(format!("{}% uploaded", (p * 100.0).round() as i64))
            }
            Self::Working { progress: None, .. } => Some("Preparing your capture…".into()),
            _ => None,
        }
    }

    /// How long a finished state stays before dismissing itself.
    pub fn auto_dismiss(&self) -> Option<Duration> {
        match self {
            Self::LinkReady { .. } => Some(Duration::from_secs(12)),
            Self::Exported { .. } => Some(Duration::from_secs(6)),
            _ => None,
        }
    }
}

/// Why Share can't start, as the deck shows it (`PreviewOverlay.share`).
pub fn share_blocked(is_configured: bool) -> TransferStatus {
    if is_configured {
        TransferStatus::failed("Uploads are off", "Turn on Upload when I share in Settings → Sharing, then try again.")
    } else {
        TransferStatus::failed("Set up cloud sharing", "Add your cloud account in Settings → Sharing, then try again.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_progress() {
        let working = TransferStatus::Working { stage: Stage::Uploading, progress: Some(0.426) };
        assert_eq!(working.compact_title(), "Uploading");
        assert_eq!(working.progress_text().as_deref(), Some("43% uploaded"));
        let preparing = TransferStatus::Working { stage: Stage::Processing, progress: None };
        assert_eq!(preparing.progress_text().as_deref(), Some("Preparing your capture…"));
        let odd = TransferStatus::Working { stage: Stage::Uploading, progress: Some(f64::NAN) };
        assert_eq!(odd.progress_text().as_deref(), Some("0% uploaded"));
        assert_eq!(TransferStatus::LinkReady { url: "x".into() }.auto_dismiss(), Some(Duration::from_secs(12)));
        assert!(share_blocked(true).compact_title() == "Share failed");
        assert!(
            matches!(share_blocked(false), TransferStatus::Failed { headline, .. } if headline == "Set up cloud sharing")
        );
    }
}
