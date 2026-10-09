#!/bin/sh
case "$*" in
  *get_config*) printf '%s' '{"config":"set $mod Mod4\nbindsym Ctrl+c exec copy\n","included_configs":[{"path":"/home/user/.config/i3/config","raw_contents":"set $mod Mod4\nbindsym Ctrl+c exec copy\n","variable_replaced_contents":"set Mod4 Mod4\nbindsym Ctrl+c exec copy\n"}]}' ;;
  *get_binding_state*) printf '%s' '{"name":"default"}' ;;
  *get_version*) printf '%s' '{"human_readable":"4.23"}' ;;
  *) exit 1 ;;
esac
