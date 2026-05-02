# COVERAGE.md — scenario manifest

Each entry maps an F-property scenario to the test that exercises it.
`scripts/manifestcheck.sh` enforces ≥3 `assert*!` calls per entry.

Format: `[F-NN] description -> test_name (assertions: status, body, state)`

## Per-endpoint validation

- [F-00] POST /users creates a user -> users_post_creates_user (assertions: status, body.handle, body.id)
- [F-00] POST /users with duplicate handle returns 409 -> users_post_duplicate_returns_409 (assertions: status, body, state)
- [F-00] POST /users with empty handle returns 400 -> users_post_empty_handle_400 (assertions: status, body, state)
- [F-04] POST /follow self-follow forbidden -> follow_self_forbidden_f4 (assertions: status, body, state)
- [F-09] POST /follow unknown user returns 400 -> follow_unknown_user_400 (assertions: status, body, state)
- [F-03] POST /follow is idempotent -> follow_idempotent_f3 (assertions: 1st status, 2nd status, 3rd status)
- [F-03] DELETE /follow on missing edge is 204 -> unfollow_idempotent_f3 (assertions: status, follow-up status, timeline state)
- [F-09] DELETE /follow unknown user returns 400 -> unfollow_unknown_user_400 (assertions: status, body, state)
- [F-00] POST /tweets with empty text returns 400 -> tweet_empty_text_400 (assertions: status, body, timeline state)
- [F-06] POST /tweets unknown author returns 400 -> tweet_unknown_author_400_f6 (assertions: status, body, follow-up state)
- [F-07] POST /tweets stamps clock and id; ties allowed -> tweet_uses_clock_and_id_f7_f8 (assertions: 1st status+body, 2nd body, monotonic id)
- [F-00] GET /timeline missing user returns 400 -> timeline_empty_user_400 (assertions: missing-user status+body, empty-user status+body)
- [F-00] GET /timeline invalid limit returns 400 -> timeline_invalid_limit_400 (assertions: negative status+body, non-numeric status+body)
- [F-01] GET /timeline unknown user returns 200 empty -> timeline_unknown_user_returns_empty_200 (assertions: status, body shape, empty array)
- [F-02] GET /timeline orders by (created desc, id desc) -> timeline_orders_by_created_then_id_f2 (assertions: status, length, ordered ids)
- [F-01] GET /timeline visibility (self + followed) -> timeline_visibility_f1 (assertions: status, length, content)
- [F-00] GET /timeline limit truncates -> timeline_limit_truncates (assertions: status, limit=2 length, limit=0 length)

## End-to-end story

- [F-01] full-flow user story: create, follow, post, read, unfollow -> full_user_story (assertions: status, body, state across 8+ steps)

## Conformance replay

- [F-00] Replay specs/conformance.jsonl byte-identically with the Go impl -> conformance_replay (assertions: status, body, state per step across 18 steps)
