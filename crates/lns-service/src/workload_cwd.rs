pub fn resolve(cli_workdir: Option<&str>, image_workdir: Option<&str>) -> Option<String> {
    cli_workdir.or(image_workdir).map(str::to_string)
}

pub fn image_workdir(image_config: Option<&oci_client::config::ConfigFile>) -> Option<String> {
    image_config
        .and_then(|c| c.config.as_ref())
        .and_then(|c| c.working_dir.clone())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_the_cli_flag_over_the_image_workdir() {
        assert_eq!(
            resolve(Some("/app"), Some("/srv")),
            Some("/app".to_string())
        );
    }

    #[test]
    fn resolve_falls_back_to_the_image_workdir() {
        assert_eq!(resolve(None, Some("/srv")), Some("/srv".to_string()));
    }

    #[test]
    fn resolve_is_none_when_neither_is_set() {
        assert_eq!(resolve(None, None), None);
    }

    fn config(working_dir: Option<&str>) -> oci_client::config::ConfigFile {
        oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            config: Some(oci_client::config::Config {
                working_dir: working_dir.map(str::to_string),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn image_workdir_reads_the_oci_working_dir() {
        assert_eq!(
            image_workdir(Some(&config(Some("/srv")))),
            Some("/srv".to_string())
        );
    }

    #[test]
    fn image_workdir_treats_an_empty_working_dir_as_unset() {
        assert_eq!(image_workdir(Some(&config(Some("")))), None);
        assert_eq!(image_workdir(Some(&config(None))), None);
    }

    #[test]
    fn image_workdir_is_none_without_an_image_config() {
        assert_eq!(image_workdir(None), None);
        let bare = oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            ..Default::default()
        };
        assert_eq!(image_workdir(Some(&bare)), None);
    }
}
