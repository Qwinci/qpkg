# qpkg
## A meta build system for os distributions
qpkg is a meta build system for os distributions, for an usage example see [crescent](https://github.com/Qwinci/crescent-bootstrap).

## Note: This isn't stable at the moment so I don't recommend using it for your own projects.

### Source format
- an url to a file e.g. `https://example.com/myfile.tar.gz`
- a git url (by default does a shallow copy)
  - `https://example.com/myrepo.git` will clone the default branch
  - `https://example.com/myrepo.git:somebranch` will clone `somebranch`
  - `https://example.com/myrepo.git:,full` will clone the default branch using a non-shallow clone
  - `https://example.com/myrepo.git:somebranch,full` will clone `somebranch` using a non-shallow clone
