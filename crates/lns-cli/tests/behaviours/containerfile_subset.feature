Feature: the Containerfile instructions lns builds
  lns builds a Containerfile itself rather than shelling out to Docker, so it
  builds a defined subset of the instruction set. `validate` reads the file the
  document names and refuses every instruction outside that subset where the
  author can still fix it — naming the instruction, the line it sits on, and
  the alternative to write instead.

  Scenario: a Containerfile of subset instructions only is valid
    Given an lns.yaml whose image is "./image"
    And the file "image/Containerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      ARG CLAUDE_CODE_VERSION=2.1.263
      ENV NODE_ENV=production
      WORKDIR /srv
      COPY ./app /srv/app
      RUN npm install -g @anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}
      USER node
      CMD ["node", "."]
      """
    When the user runs artifact command "validate"
    Then the exit code is 0
    And the output contains "valid"

  Scenario: a Dockerfile is read under that name too
    Given an lns.yaml whose image is "./image"
    And the file "image/Dockerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      RUN npm ci
      """
    When the user runs artifact command "validate"
    Then the exit code is 0
    And the output contains "valid"

  Scenario: a HEALTHCHECK is refused with its line and where the subset grows
    Given an lns.yaml whose image is "./image"
    And the file "image/Containerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      RUN npm ci
      HEALTHCHECK CMD curl -f http://localhost/
      """
    When the user runs artifact command "validate"
    Then the exit code is 1
    And the output contains "./image/Containerfile"
    And the output contains "line 3"
    And the output contains "HEALTHCHECK"
    And the output contains "no alternative in v1"

  Scenario: a second stage is refused with the one-stage alternative
    Given an lns.yaml whose image is "./image"
    And the file "image/Containerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm AS build
      RUN npm ci
      FROM docker.io/library/node:24-bookworm
      COPY --from=build /srv /srv
      """
    When the user runs artifact command "validate"
    Then the exit code is 1
    And the output contains "line 3"
    And the output contains "name it in FROM"

  Scenario: a COPY whose source leaves the context is refused naming that source
    Given an lns.yaml whose image is "./image"
    And the file "image/Containerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      COPY ../secrets/token /srv/token
      """
    When the user runs artifact command "validate"
    Then the exit code is 1
    And the output contains "line 2"
    And the output contains "../secrets/token"
    And the output contains "leaves the build context"

  Scenario: a Containerfile that does not parse is refused with the line it fails on
    Given an lns.yaml whose image is "./image"
    And the file "image/Containerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      RUN npm ci
      INSTALL everything
      """
    When the user runs artifact command "validate"
    Then the exit code is 1
    And the output contains "line 3"
    And the output contains "INSTALL"
