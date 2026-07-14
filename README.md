# Biscuit datalog language server

## Introduction

This repo provides a language server for biscuit datalog.

## Demo

[![asciicast](https://asciinema.org/a/1260820.svg)](https://asciinema.org/a/1260820)

## Features

- syntactic error diagnostic
- code completion
- find reference
- rename
- go to definition
- code actions
  - add `trusting` clause
  - convert `check` blocks
  - convert `policy` blocks
  - replace literal with parameter
  - replace parameter with current date
  - extract rule body
  - inline rule body
  - sort rule body (predicates first, expressions second)

## Installation / usage

This language server has not been released yet and is not part of the [biscuit-auth VSCode extension](https://marketplace.visualstudio.com/items?itemName=biscuit-auth.vscode-biscuit).

You can build the binary locally (with `cargo install --path .`) and configure your editor to use it:

### Helix

First, make sure the `biscuit` language is configured in helix: <https://github.com/eclipse-biscuit/tree-sitter-biscuit#helix>.

Then, update the `languages.toml` config file to:

- add `biscuit-language-server` to biscuit’s `language-servers`;
- add a new entry in `[language-server]`

```toml
[[language]]
name = "biscuit"
…
language-servers = ["biscuit-language-server"]

[language-server]
…
biscuit-language-server = { command = "biscuit-language-server" }
```
