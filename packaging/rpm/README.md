# RPM metadata

RPM spec files and related package metadata belong here. Keep them declarative:
use native RPM macros and package metadata, not embedded shell, Python, Ruby, or
other custom implementations. Build behavior must come from the Rust workspace
or retained upstream components. Package names, dependencies, scriptlets, and
service policy are introduced with the task that implements each capability.
