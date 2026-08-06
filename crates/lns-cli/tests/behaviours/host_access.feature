Feature: lns run brings the host's signing identity into the sandbox (host access)
  A published sandbox definition cannot name a host path, so it declares an
  intent by id — `spec.hostAccess: [git-signing]` — and lns resolves the
  host-specific half per user: the resolved git config, the public keyring, and
  the signing agent's socket. Declaring never arms, the same way a declared
  connector never arms; the consumer's `lns-policy.yaml` grants it per directory,
  and an ungranted id raises a card at launch. The host's own `commit.gpgsign`
  decides whether the capability is mandatory: enabled and unavailable refuses
  the launch before any microVM boots, disabled forwards and projects nothing.
  These scenarios pin the host resolution, the grant flow, the secret scan over
  config keys, and the run-summary surface against the in-process resolver with a
  faked host-command runner; the signature itself is guest-observable and lives
  in the @microvm e2e contract.

  Scenario: A host that does not sign runs unsigned
    Given the sandbox definition declares host access "git-signing"
    And the host git config leaves commit.gpgsign off
    When the host access is resolved for `lns run alpine` interactively
    Then no host-access card is shown
    And no agent socket is forwarded
    And no git config is projected
    And the host-access summary shows "git-signing: absent (host does not sign)"

  Scenario: A host that signs projects its identity and forwards its agent
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host git config sets user.email to "me@example.test"
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    And the operator will answer the host-access card with "grant"
    When the host access is resolved for `lns run alpine` interactively
    Then the projected git config carries "user.email=me@example.test"
    And the forwarded agent socket is "/run/user/501/gnupg/S.gpg-agent.extra"
    And the host-access summary shows a line "git-signing → identity + agent"

  Scenario: The host requires signing but no agent can be located
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And no agent socket can be located on the host
    When the host access is resolved for `lns run alpine` interactively
    Then the host-access resolution fails with "no signing agent"
    And the failure names "commit.gpgsign" as the setting that required it
    And the sandbox is not launched

  Scenario: A host with no GnuPG at all runs the definition with the capability absent
    Given the sandbox definition declares host access "git-signing"
    And the host has no git config and no agent
    When the host access is resolved for `lns run alpine` interactively
    Then the host-access resolution succeeds
    And the host-access summary shows "git-signing: absent (host does not sign)"

  Scenario: The repository's own setting wins over the global one
    Given the sandbox definition declares host access "git-signing"
    And the host git config disables commit.gpgsign globally
    And the repository at the working directory enables commit.gpgsign
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    And the operator will answer the host-access card with "grant"
    When the host access is resolved for `lns run alpine` interactively
    Then the forwarded agent socket is "/run/user/501/gnupg/S.gpg-agent.extra"

  Scenario: Granting the card records the grant in this directory's policy
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    And the operator will answer the host-access card with "grant"
    When the host access is resolved for `lns run alpine` interactively
    Then the directory's policy records host access "git-signing"

  Scenario: A recorded grant arms without a card
    Given the sandbox definition declares host access "git-signing"
    And the directory's policy already records host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    When the host access is resolved for `lns run alpine` interactively
    Then no host-access card is shown
    And the forwarded agent socket is "/run/user/501/gnupg/S.gpg-agent.extra"

  Scenario: A directory with no definition can grant host access
    Given the directory has no sandbox definition
    And the directory's policy already records host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    When the host access is resolved for `lns run alpine` interactively
    Then the forwarded agent socket is "/run/user/501/gnupg/S.gpg-agent.extra"

  Scenario: Declining the card refuses the launch when the host requires signing
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    And the operator will answer the host-access card with "decline"
    When the host access is resolved for `lns run alpine` interactively
    Then the host-access resolution fails with "host access declined"
    And the sandbox is not launched
    And the directory's policy records no host access
    And a per-machine decline is now recorded for host access "git-signing"

  Scenario: A remembered decline refuses the next run without asking again
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And a per-machine decline is recorded for host access "git-signing"
    When the host access is resolved for `lns run alpine` interactively
    Then no host-access card is shown
    And the host-access resolution fails with "host access declined"

  Scenario: A standing decline outranks a policy grant that arrived with a clone
    Given the sandbox definition declares host access "git-signing"
    And the directory's policy already records host access "git-signing"
    And a per-machine decline is recorded for host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    When the host access is resolved for `lns run alpine` interactively
    Then the host-access resolution fails with "standing decline"
    And no agent socket is forwarded

  Scenario: A non-interactive run with no grant refuses rather than asking
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And the directory's policy records no host access
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    When the host access is resolved for `lns run -d alpine` with no terminal
    Then the host-access resolution fails with "no terminal to confirm"
    And the failure names "lns host-access grant" as the fix
    And no host-access card is shown

  Scenario: A secret-shaped setting is dropped from the projected config
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host git config sets "http.https://git.example.test/.extraheader"
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    And the operator will answer the host-access card with "grant"
    And the operator will answer the config secret prompt with "drop"
    When the host access is resolved for `lns run alpine` interactively
    Then the projected git config omits "http.https://git.example.test/.extraheader"
    And a per-machine DROP decision is recorded for the config key "http.https://git.example.test/.extraheader"
    And the host-access summary shows "http.https://git.example.test/.extraheader: dropped"
    And a later run with the same host config shows no config secret prompt

  Scenario: Keeping a secret-shaped setting projects it after all
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host git config sets "sendemail.smtppass"
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    And the operator will answer the host-access card with "grant"
    And the operator will answer the config secret prompt with "keep"
    When the host access is resolved for `lns run alpine` interactively
    Then the projected git config carries "sendemail.smtppass=some-host-secret"
    And a per-machine KEEP decision is recorded for the config key "sendemail.smtppass"

  Scenario: A non-interactive run drops an undecided secret-shaped setting
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host git config sets an undecided "sendemail.smtppass"
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    And the directory's policy already records host access "git-signing"
    When the host access is resolved for `lns run -d alpine` with no terminal
    Then the projected git config omits "sendemail.smtppass"
    And the dropped config key "sendemail.smtppass" is reported on stderr
    And no config secret prompt is shown

  Scenario: Includes are resolved on the host so the guest sees one flat config
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host git config includes a file setting user.email to "work@example.test"
    And the host agent socket is located at "/run/user/501/gnupg/S.gpg-agent.extra"
    And the operator will answer the host-access card with "grant"
    When the host access is resolved for `lns run alpine` interactively
    Then the projected git config carries "user.email=work@example.test"
    And the projected git config names no host include path

  Scenario: An ssh-format signing host locates the ssh agent instead
    Given the sandbox definition declares host access "git-signing"
    And the host git config enables commit.gpgsign
    And the host git config sets gpg.format to "ssh"
    And the host ssh agent socket is located at "/run/user/501/ssh-agent.sock"
    And the operator will answer the host-access card with "grant"
    When the host access is resolved for `lns run alpine` interactively
    Then the forwarded agent socket is "/run/user/501/ssh-agent.sock"

  Scenario: An unknown host-access id refuses the launch
    Given the sandbox definition declares host access "not-in-catalog"
    When the host access is resolved for `lns run alpine` interactively
    Then the host-access resolution fails with "unknown host access"
    And the sandbox is not launched
