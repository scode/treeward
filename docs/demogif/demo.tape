# VHS tape for treeward demo

Output demo.gif

Set FontSize 14
Set Width 1000
Set Height 600
Set Theme "Catppuccin Mocha"

Type@0ms "# Initialize a directory with treeward"
Enter
Sleep 500ms
Type@0ms "treeward init"
Enter
Sleep 1s

Enter
Type@0ms "# Check the status - should be clean"
Enter
Sleep 500ms
Type "treeward status"
Enter
Sleep 1s

Enter
Type@0ms "# What files are we tracking?"
Enter
Sleep 500ms
Type "treeward status --all"
Enter
Sleep 1s

Enter
Type@0ms "# Let's modify something"
Enter
Sleep 500ms
Type "echo hey > subdir/hey.txt"
Enter
Sleep 1s

Enter
Type@0ms "# We have added a file"
Enter
Sleep 500ms
Type "treeward status"
Enter
Sleep 1s

Enter
Type@0ms "# Let's update the ward"
Enter
Sleep 500ms
Type "treeward update"
Enter
Sleep 1s

Enter
Type@0ms "# Now we are clean again"
Enter
Sleep 500ms
Type "treeward status"
Enter
Sleep 1s
