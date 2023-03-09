#!/bin/bash
set -e
VERSION=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0] | [ .version ] | join("-")')

IMG="slack-app-folder-echo-build"

if [[ ! " $@ " =~ " --skip-docker-build " ]]; then
    echo "=========== Build docker image ==========="
    docker build -t $IMG .
fi

echo "=========== Make .exe ==========="
docker run --rm -iv${PWD}:/root/OUTPUT $IMG bash -xvs << EOF
    set -e
    cd /root
    cargo --verbose build --target x86_64-pc-windows-gnu --release --verbose || exit 1
    chown -v $(id -u):$(id -g)  target/x86_64-pc-windows-gnu/release/*.exe
    cp -va target/x86_64-pc-windows-gnu/release/slack-app-folder-echo.exe OUTPUT/slack-app-folder-echo-${VERSION}.exe
    echo "============ Done. Built for: ============="
    lsb_release -a
EOF

echo "=========== Make .deb ==========="
docker run --rm -iv${PWD}:/root/OUTPUT $IMG bash -xvs << EOF
    set -e
    cd /root
    cargo --verbose deb --verbose || exit 1
    chown -v $(id -u):$(id -g) target/debian/*.deb
    cp -va target/debian/*.deb OUTPUT/
    cp -va target/release/slack-app-folder-echo OUTPUT/
    echo "============ Done. Built for: ============="
    lsb_release -a
    echo "\n...and x86_64-pc-windows-gnu"
EOF

echo "=============== $(pwd) ==============="
ls -l *.deb *.exe
