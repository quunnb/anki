# Anki®

## Let my spaces be

This is my personal fork of [Anki](https://github.com/ankitects/anki). Anki has a nice [feature](https://docs.ankiweb.net/templates/fields.html#checking-your-answer) that lets you type in your answer, and then highlights any errors you made. This is great for language learning, but one thing I also use it for is remembering keyboard shortcuts. In [Doom Emacs](https://github.com/doomemacs/doomemacs) and in [my vimrc](https://github.com/quunnb/dotfiles/blob/main/.vim/vimrc) a leader key is used to prefix all kinds of useful shortcuts, and that leader key is space. Unfortunately, and perhaps reasonably, Anki trims all whitespace from the answer it expects, so on the answer card all the inputted leading space characters are shown as errors. This "build" fixes that. Related tests are commented out so that all tests continue to pass.
