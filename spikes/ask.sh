#!/bin/bash
# usage: ask.sh <session> <text>   types text, waits, presses Enter as separate write (tmux paste-burst detection otherwise eats Enter)
tmux -L spikes send-keys -t $1 "$2"; sleep 1; tmux -L spikes send-keys -t $1 C-m
