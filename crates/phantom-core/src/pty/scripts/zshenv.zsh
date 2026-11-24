# terax-shell-integration (zshenv)
#
# Trailing `:` is load-bearing — without it, a missing user .zshenv leaves $?=1,
# which propagates through the rest of init and ultimately into the first
# prompt's `%?` (rendering robbyrussell's `➜` red on a clean shell start).
{
  # Claude Code tags its child shells with CLAUDE_CODE_* markers, which can reach
  # this shell via the multiplexer daemon's inherited environment. Left in place
  # a `claude` started here treats itself as a child session and won't persist,
  # so strip the family before anything runs.
  unset -m 'CLAUDE_CODE_*'
  _terax_user_zdotdir="${TERAX_USER_ZDOTDIR:-$HOME}"
  [ -f "$_terax_user_zdotdir/.zshenv" ] && source "$_terax_user_zdotdir/.zshenv"
  unset _terax_user_zdotdir
}
:
