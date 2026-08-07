Feature: HOME and USER when the run-as user is not the author's user
  A workload's HOME and USER come from the run-as user's guest passwd
  entry. That is the right default, but it silently outranked what the
  author declared: a definition setting HOME=/home/sandbox, run with
  `-u root`, gave the workload HOME=/root and said nothing. What the
  author declares now wins, and an image's own ENV HOME does not — an
  image shipping ENV HOME=/root must never hand an unprivileged
  workload a home it cannot write.

  Scenario: A declared HOME is pinned through the supervised workload env
    Given the definition declares env "HOME=/home/sandbox"
    When the user runs `lns run someimage` under a policy
    Then the supervised workload env pins the declared HOME to "/home/sandbox"

  Scenario: -e declares a HOME just as the definition does
    When the user runs `lns run -e HOME=/home/sandbox someimage` under a policy
    Then the supervised workload env pins the declared HOME to "/home/sandbox"

  Scenario: A declared USER is pinned the same way
    Given the definition declares env "USER=builder"
    When the user runs `lns run someimage` under a policy
    Then the supervised workload env pins the declared USER to "builder"

  Scenario: An image's ENV HOME is not a declaration and stays subordinate
    Given the image declares ENV HOME=/root
    When the user runs `lns run someimage` under a policy
    Then the supervised workload env pins no declared HOME

  Scenario: An image cannot forge the declaration itself
    Given the image declares ENV LENS_SANDBOX_WORKLOAD_HOME=/root
    When the user runs `lns run someimage` under a policy
    Then the supervised workload env pins no declared HOME

  Scenario: -e cannot forge the declaration either
    When the user runs `lns run -e LENS_SANDBOX_WORKLOAD_HOME=/root someimage` under a policy
    Then the supervised workload env pins no declared HOME

  Scenario: A run that declares nothing keeps the run-as user's own home
    When the user runs `lns run someimage` under a policy
    Then the supervised workload env pins no declared HOME

  Scenario: An unsupervised run needs no marker, because nothing rewrites its HOME
    Given the definition declares env "HOME=/home/sandbox"
    When the user runs `lns run someimage` without a policy
    Then the supervised workload env pins no declared HOME
