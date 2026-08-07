@microvm
Feature: host access forwards the host's signing agent into the guest
  A signature is the only proof the forward works: the guest must hold the public
  key as a stub, reach the host agent over vsock, and produce a signature that
  verifies — while the private key stays on the host. The socket appears at the
  run-as user's home, owned by that user with mode 0600, so who runs the workload
  decides where it lands. These outcomes are guest-observable and need a booted
  microVM plus a live host agent, so they are @microvm and run only via
  `make e2e-microvm`.

  Scenario: a commit made in the guest carries a signature that verifies
    Given the Lens Sandbox service is running
    And the host agent holds the key named by user.signingkey
    When the user runs a microVM command "/bin/sh -c 'apk add --no-cache gnupg git >/dev/null; mkdir -p /tmp/r && cd /tmp/r && git init -q . && git commit --allow-empty -S -m x && git log --show-signature -1'" as user "root" with host access "git-signing"
    Then the exit code is 0
    And the output contains "Good signature"

  Scenario: the verified signature reports full trust, not unknown validity
    Given the Lens Sandbox service is running
    And the host agent holds the key named by user.signingkey
    When the user runs a microVM command "/bin/sh -c 'apk add --no-cache gnupg git >/dev/null; mkdir -p /tmp/r && cd /tmp/r && git init -q . && git commit --allow-empty -S -m x && echo trust=$(git log --format=%G? -1)'" as user "root" with host access "git-signing"
    Then the exit code is 0
    And the output contains "trust=G"

  Scenario: the guest holds the signing key as a stub with no private material
    Given the Lens Sandbox service is running
    And the host agent holds the key named by user.signingkey
    When the user runs a microVM command "/bin/sh -c 'apk add --no-cache gnupg >/dev/null; gpg --batch --list-secret-keys'" as user "root" with host access "git-signing"
    Then the exit code is 0
    And the output contains "sec#"

  Scenario: the private key cannot be exported through the forwarded agent
    Given the Lens Sandbox service is running
    And the host agent holds the key named by user.signingkey
    When the user runs a microVM command "/bin/sh -c 'apk add --no-cache gnupg >/dev/null; gpg --batch --export-secret-keys > /dev/null 2>&1; echo rc=$?'" as user "root" with host access "git-signing"
    Then the output contains "rc=2"

  Scenario: the forwarded socket belongs to the run-as user
    Given the Lens Sandbox service is running
    And the host agent holds the key named by user.signingkey
    When the user runs a microVM command "/bin/sh -c 'ls -ln $HOME/.gnupg/S.gpg-agent'" with host access "git-signing"
    Then the exit code is 0
    And the output shows the socket owned by the run-as user with mode 0600

  Scenario: the forwarded socket lands in root's home when the run-as user is root
    Given the Lens Sandbox service is running
    And the host agent holds the key named by user.signingkey
    When the user runs a microVM command "/bin/sh -c 'ls /root/.gnupg/S.gpg-agent'" as user "root" with host access "git-signing"
    Then the exit code is 0
    And the output contains "/root/.gnupg/S.gpg-agent"

  Scenario: two signs at once each get their own connection
    Given the Lens Sandbox service is running
    And the host agent holds the key named by user.signingkey
    When the user runs a microVM command "/bin/sh -c 'apk add --no-cache gnupg >/dev/null; echo a > /tmp/a; echo b > /tmp/b; gpg --batch --yes --detach-sign /tmp/a & gpg --batch --yes --detach-sign /tmp/b; wait; gpg --verify /tmp/a.sig /tmp/a && gpg --verify /tmp/b.sig /tmp/b'" as user "root" with host access "git-signing"
    Then the exit code is 0

  Scenario: the sandbox survives the host agent stopping mid-run
    Given the Lens Sandbox service is running
    And the host agent holds the key named by user.signingkey
    And a sandbox is running with host access "git-signing"
    When the host agent stops
    And the workload attempts a signature
    Then the signature attempt fails
    And the sandbox is still running
