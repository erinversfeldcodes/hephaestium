#!/usr/bin/env bash
# The names a pull request's preview is given, derived from its branch alone, so the deploy,
# the teardown and the sweep always agree on what belongs to which branch.
#
# A Fly app name is at most 30 characters; "heph-cp-pr-" is 11, so the branch part is cut to
# 19. A run-unique suffix (PREVIEW_SUFFIX) takes part of that budget, so two runs for one branch
# cannot collide.

_preview_sanitise() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9\n' '-' | tr -s '-'
}

# derive_preview_names <branch>: sets PREVIEW_COMPONENT, PREVIEW_APP and PREVIEW_NEON_BRANCH.
derive_preview_names() {
  local branch=$1 budget=19 sanitised suffix part
  sanitised=$(_preview_sanitise "$branch")
  sanitised=${sanitised#-}
  if [[ -n ${PREVIEW_SUFFIX:-} ]]; then
    suffix=$(_preview_sanitise "$PREVIEW_SUFFIX")
    suffix=${suffix#-}
    suffix=${suffix%-}
    local room=$((budget - ${#suffix} - 1))
    if ((room < 1)); then
      echo "FAIL preview-names: PREVIEW_SUFFIX '${suffix}' is too long: at most $((budget - 2)) characters" >&2
      return 1
    fi
    part=$(printf '%s' "$sanitised" | cut -c1-"$room")
    part=${part%-}
    PREVIEW_COMPONENT="${part}-${suffix}"
  else
    part=$(printf '%s' "$sanitised" | cut -c1-"$budget")
    PREVIEW_COMPONENT=${part%-}
  fi
  if [[ -z $PREVIEW_COMPONENT ]]; then
    echo "FAIL preview-names: nothing is left of branch '${branch}' to name a preview by" >&2
    return 1
  fi
  # shellcheck disable=SC2034  # read by the scripts that source this file
  PREVIEW_APP="heph-cp-pr-${PREVIEW_COMPONENT}"
  # shellcheck disable=SC2034
  PREVIEW_NEON_BRANCH="preview/${PREVIEW_COMPONENT}"
}

# The pattern every preview Fly app matches, for the sweep.
# shellcheck disable=SC2034  # read by the sweep
PREVIEW_APP_PATTERN='^heph-cp-pr-'
