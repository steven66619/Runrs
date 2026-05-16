#!/bin/bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")" && pwd)"
ARCH_DIR="$REPO_DIR/arch/x86_64"

build_and_update() {
    cd "$REPO_DIR/.."

    echo "==> Building package..."
    makepkg --skipinteg --noconfirm

    echo "==> Copying package to repo..."
    mkdir -p "$ARCH_DIR"
    cp *.pkg.tar.zst "$ARCH_DIR/"

    echo "==> Updating repo database..."
    cd "$ARCH_DIR"
    repo-add launcher.db.tar.zst *.pkg.tar.zst

    # GitHub Pages doesn't follow symlinks, so replace with actual copies
    rm -f launcher.db launcher.files
    cp launcher.db.tar.zst launcher.db
    cp launcher.files.tar.zst launcher.files

    echo "==> Done. Ready for GitHub Pages deployment."
    echo "    Push repo/ directory to gh-pages branch:"
    echo "    git add repo/ && git commit -m 'Update repo' && git push origin master"
    echo "    git subtree push --prefix repo origin gh-pages"
}

case "${1:-}" in
    ""|build)
        build_and_update
        ;;
    deploy)
        echo "==> Deploying to GitHub Pages..."
        git subtree push --prefix repo origin gh-pages
        ;;
    *)
        echo "Usage: $0 [build|deploy]"
        exit 1
        ;;
esac
