Feature: lns run falls back to configured defaults
  Defaults stored with `lns config` apply to `lns run` whenever the
  matching per-run flag is absent. A per-run flag always wins — exactly
  for single-value settings, and entry-by-entry for env / volume /
  publish lists, so one override never silently drops the other
  configured defaults.

  Scenario: Configured resources apply when no flag is passed
    Given the default "run.cpus" is "4"
    And the default "run.mem" is "2048"
    When the user resolves `lns run alpine` against the configured defaults
    Then the run summary shows "4 vCPU · 2048 MiB"

  Scenario: A per-run flag overrides the configured resource default
    Given the default "run.cpus" is "4"
    When the user resolves `lns run --cpus 2 alpine` against the configured defaults
    Then the run summary shows "2 vCPU · 512 MiB"

  Scenario: Built-in defaults apply when nothing is configured
    When the user resolves `lns run alpine` against the configured defaults
    Then the run summary shows "1 vCPU · 512 MiB"

  Scenario: Configured env defaults are injected into the run
    Given the default "run.env" is "TZ=UTC CI=1"
    When the user resolves `lns run alpine` against the configured defaults
    Then the resolved env is exactly "TZ=UTC, CI=1"

  Scenario: A per-run -e overrides only the matching configured variable
    Given the default "run.env" is "TZ=UTC CI=1"
    When the user resolves `lns run -e TZ=EET alpine` against the configured defaults
    Then the resolved env is exactly "CI=1, TZ=EET"

  Scenario: Configured volumes mount alongside per-run volumes
    Given the default "run.volume" is "cache:/var/cache"
    When the user resolves `lns run -v scratch:/tmp/scratch alpine` against the configured defaults
    Then the resolved volumes are exactly "cache:/var/cache, scratch:/tmp/scratch"

  Scenario: A per-run -v overrides the configured volume at the same target
    Given the default "run.volume" is "cache:/var/cache"
    When the user resolves `lns run -v fresh:/var/cache alpine` against the configured defaults
    Then the resolved volumes are exactly "fresh:/var/cache"

  Scenario: Configured ports publish alongside per-run ports
    Given the default "run.publish" is "8080:80"
    When the user resolves `lns run -p 9090:90 alpine` against the configured defaults
    Then the resolved ports are exactly "127.0.0.1:8080->80, 127.0.0.1:9090->90"

  Scenario: A per-run -p overrides the configured publish on the same host bind
    Given the default "run.publish" is "8080:80"
    When the user resolves `lns run -p 8080:3000 alpine` against the configured defaults
    Then the resolved ports are exactly "127.0.0.1:8080->3000"

  Scenario: A hand-edited config with an invalid default fails the run naming the file
    Given the config file declares a malformed "run.env" entry "BARE"
    When the user resolves `lns run alpine` against the configured defaults
    Then the resolution fails mentioning "run.env" and the config file
