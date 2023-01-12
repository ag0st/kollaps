# How to install
```shell
LLVM_SYS_13_PREFIX=/usr/include/llvm13/ cargo install cargo-bpf --no-default-features --features=llvm13,command-line
```
# How to build
```shell
LLVM_SYS_13_PREFIX=/usr/include/llvm13/ cargo bpf build
```
