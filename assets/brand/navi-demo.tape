# Real TTY recording of the NAVI TUI (no mouse overlay).
# Free model only: opencode / mimo-v2.5-free (no Charm Hyper).
#
#   just record-demo
#   # or: bash scripts/record-demo.sh

Output navi-demo.gif
Output navi-demo.mp4

Set Shell "bash"
Set FontSize 15
Set Width 1600
Set Height 960
Set Padding 28
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set WindowBarSize 40
Set TypingSpeed 34ms
Set Framerate 14
Set PlaybackSpeed 1.0
Set LetterSpacing 0

Hide
Type "cd /tmp/navi-gif-demo && clear && /home/enrell/.local/bin/navi --yolo"
Enter
Sleep 2.8s
Show

# t≈0 welcome
Sleep 2.5s

# t≈2.5 Commands palette
Ctrl+P
Sleep 1.3s

# t≈3.8 Models
Type "Models"
Sleep 700ms
Enter
Sleep 1.5s

# t≈6.0 free model only — exact free id so we never pick paid/Command Code
Type "mimo-v2.5-free"
Sleep 1.1s
Enter
Sleep 1.6s

# t≈8.7 type prompt
Type "Summarize README.md in one short sentence."
Sleep 1.0s

# t≈13 send
Ctrl+Enter
Sleep 14s

# t≈27 linger
Sleep 3.2s

# t≈30.2 Usage feature
Ctrl+P
Sleep 800ms
Type "Usage"
Sleep 600ms
Enter
Sleep 2.2s
Escape
Sleep 600ms

Ctrl+C
Sleep 400ms
