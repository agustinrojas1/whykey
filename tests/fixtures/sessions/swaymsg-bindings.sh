#!/bin/sh
case "$*" in
  *get_config*) printf '%s' '{"config":"set $mod Mod4\nbindsym Ctrl+c exec copy\nbindsym $mod+Return exec foot\n"}' ;;
  *get_binding_state*) printf '%s' '{"name":"default"}' ;;
  *get_version*) printf '%s' '{"human_readable":"1.10"}' ;;
  *) exit 1 ;;
esac
