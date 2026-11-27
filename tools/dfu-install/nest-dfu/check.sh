#!/bin/sh
# Check the loader images in this directory against the checksums the install was tested with.
# The images are not in the repository: see README.md for where they come from.
cd "$(dirname "$0")" || exit 1
bad=0
check() {
    if [ ! -f "$1" ]; then echo "missing    $1"; bad=1; return; fi
    have=$(sha256sum "$1" | cut -d' ' -f1)
    if [ "$have" = "$2" ]; then echo "ok         $1"; else echo "different  $1 (sha256 $have)"; bad=1; fi
}
check x-load.bin   aee635f0d09180677fa86fea6797b1ad36f64b112f7eea3a5572ab7f28fd5c21
check u-boot.bin   52aa039f5b36d8cd5108e6bbb69f5dbd60a5c3750404c8d3fbc721d53721ccf0
check rescueUImage add18697d358c8d15f6b8ad0dcd6a6cb926ecb6e6a9ce490573b91c0e148355f
if [ $bad = 1 ]; then
    echo "An image you built yourself has another checksum; that is not an error by itself,"
    echo "but the install was tested with the images above only."
fi
exit $bad
