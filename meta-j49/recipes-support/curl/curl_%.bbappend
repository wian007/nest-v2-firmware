# curl only for j49-update (https download of a new root file system): mbedtls instead of
# OpenSSL, which the image does not carry, and nothing that is not needed for a plain download.
PACKAGECONFIG:class-target = "mbedtls proxy threaded-resolver zlib ${@bb.utils.filter('DISTRO_FEATURES', 'ipv6', d)}"
