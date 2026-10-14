# Terminal demo for the README. Regenerate with:  make demo
# Records the real CLI against the data in data/; nothing is staged.

Output docs/demo.gif

Set Shell "bash"
Set FontSize 14
Set Width 1180
Set Height 560
Set Padding 18
Set Theme "GitHub Dark"
Set TypingSpeed 35ms

Hide
Type "export PATH=/opt/tsz:$PATH PS1='$ ' && mkdir -p /tmp/d && cd /tmp/d && for f in /vhs/data/*.csv.gz; do zcat $f > $(basename $f .gz); done && clear"
Enter
Show

Type "tsz compare *.csv"
Enter
Sleep 6s

Type "clear"
Enter
Type "tsz encode uci-power-active-2007.csv -o power.tsz"
Enter
Sleep 3s

Type "tsz inspect power.tsz"
Enter
Sleep 3s

Type "tsz decode power.tsz | head -3"
Enter
Sleep 2s

Type "tsz verify noaa-sf-water-level.csv"
Enter
Sleep 4s
