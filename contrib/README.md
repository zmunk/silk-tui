# Silk Shell Integration

A ZLE widget for launching Silk from zsh.

## Quick install

```sh
./install.sh
```

This copies `silk-widget.zsh` into `~/.local/share/silk/` and adds a
`source …` line to `~/.zshrc`.

## Manual install

Source the widget in your `.zshrc`:

```sh
source /path/to/silk/contrib/silk-widget.zsh
```

Then bind it to a key of your choosing:

```sh
bindkey '^X^S' silk-widget
```

## Override install directory

```sh
SILK_HOME=$XDG_DATA_HOME/silk ./install.sh
```

## Uninstall

Remove the widget file and the `source` line from `~/.zshrc`:

```sh
rm ~/.local/share/silk/silk-widget.zsh
sed -i '/silk-widget/d' ~/.zshrc
```
