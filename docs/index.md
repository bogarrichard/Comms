# Comms

A fast, keyboard-driven terminal client for **Microsoft Teams** (chats and team channels) and **Signal** (text), written in Rust on a provider-independent core. Each connection has its own colour in the interface: Teams is purple, Signal is blue.

```{toctree}
:caption: Using Comms
:maxdepth: 1

getting-started
setup-entra
signal
```

```{toctree}
:caption: Developing
:maxdepth: 1

architecture
development
microsoft-graph
```

```{toctree}
:caption: Project
:maxdepth: 1

phase-0-plan
```

:::{note}
Comms has not yet been run against a live Microsoft tenant or a live `signal-cli`; your first sign-in is the first live test. See the known limitations in the [README](https://github.com/bogarrichard/Comms#known-limitations).
:::
