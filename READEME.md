# Launcher

A tui app launcher for MacOS written in rust.

## Features
* Fuzzy searches apps in common application locations in MacOS and binaries in $PATH
* Responsive UI, high searching performance
* Does not index files at the background.
* Opens browser and search query if there is no match
* Opens URL in browser directly

## Usage
As **Launcher** does not listen to shortcut keys to start, it is best to use **Launcher** with **skhd** and **alacritty**

Example **skhd** rule

`alt + shift - p : alacritty -e bash -lc /path/to/launcher`

## Todo list
- [ ] add shortcut commands
- [ ] finish find command to find + open files
- [ ] write a UI with iced instead of using terminal --> how to open terminal to run command?
