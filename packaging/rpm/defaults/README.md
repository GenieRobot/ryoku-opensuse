# Generic port defaults

This is newly authored declarative data for the Ryoku openSUSE port, licensed
under the repository's GPL-3.0-only license. It is not copied from the upstream
Ryoku profile UI format. It uses the port's schema-1 `ProfileDocument` and its
three native typed settings: Hyprland, Ryoku dark theme, comfortable density.

The `ryoku-profile-defaults` noarch RPM owns the immutable file at
`/usr/share/ryoku/config/profile.json`, root:root mode 0644. The package contains
no executable scripts or activation hooks and never changes personal files.
System, companion and personal layers are optional local overrides. Package
reinstallation replaces only the package default; existing overrides remain
outside its file list.

A source file in this repository is not proof of an installed package.
Package file digests and ownership are recorded in the native build inventory;
configuration capture and import are implemented separately under issue #18.
