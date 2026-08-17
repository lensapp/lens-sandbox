use objc2_virtualization::{VZDiskImageCachingMode, VZDiskImageSynchronizationMode};

#[derive(Clone, Copy, Debug)]
pub(crate) enum DiskRole {
    UpperRw,
    ComposefsDescriptorRo,
    VolumeRw,
}

pub(crate) struct AttachmentPolicy {
    pub read_only: bool,
    pub caching: VZDiskImageCachingMode,
    pub synchronization: VZDiskImageSynchronizationMode,
}

pub(crate) fn policy(role: DiskRole) -> AttachmentPolicy {
    AttachmentPolicy {
        read_only: matches!(role, DiskRole::ComposefsDescriptorRo),
        caching: VZDiskImageCachingMode::Cached,
        synchronization: VZDiskImageSynchronizationMode::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLES: [DiskRole; 3] = [
        DiskRole::UpperRw,
        DiskRole::ComposefsDescriptorRo,
        DiskRole::VolumeRw,
    ];

    #[test]
    fn every_sparse_image_reads_through_the_host_page_cache() {
        for role in ROLES {
            assert_eq!(
                policy(role).caching,
                VZDiskImageCachingMode::Cached,
                "{role:?} must read through the host page cache; both .automatic and .uncached \
                 read past it and serve stale-or-zero blocks from a sparse image (issue #247)"
            );
        }
    }

    #[test]
    fn every_disk_keeps_the_guest_flush_durable() {
        for role in ROLES {
            assert_eq!(
                policy(role).synchronization,
                VZDiskImageSynchronizationMode::Full,
                "{role:?} must keep guest journal barriers backed by F_FULLFSYNC"
            );
        }
    }

    #[test]
    fn only_the_composefs_descriptor_is_attached_read_only() {
        assert!(policy(DiskRole::ComposefsDescriptorRo).read_only);
        assert!(!policy(DiskRole::UpperRw).read_only);
        assert!(!policy(DiskRole::VolumeRw).read_only);
    }
}
