# NixOS: grant the logged-in user raw USB access to the OMAP3 mask-ROM in USB
# peripheral boot, so omap-usbboot.py can run WITHOUT sudo.
#
# Add this to /etc/nixos/configuration.nix (merge into an existing
# services.udev.extraRules if you already have one), then:
#   sudo nixos-rebuild switch
#
# TAG+="uaccess" gives the active-seat user access; GROUP/MODE is a fallback
# for members of plugdev.
{
  services.udev.extraRules = ''
    SUBSYSTEM=="usb", ATTR{idVendor}=="0451", ATTR{idProduct}=="d00e", TAG+="uaccess", MODE="0660", GROUP="plugdev"
    SUBSYSTEM=="usb", ATTR{idVendor}=="0451", ATTR{idProduct}=="d00f", TAG+="uaccess", MODE="0660", GROUP="plugdev"
    SUBSYSTEM=="usb", ATTR{idVendor}=="0451", ATTR{idProduct}=="d010", TAG+="uaccess", MODE="0660", GROUP="plugdev"
    SUBSYSTEM=="usb", ATTR{idVendor}=="0451", ATTR{idProduct}=="d011", TAG+="uaccess", MODE="0660", GROUP="plugdev"
    SUBSYSTEM=="usb", ATTR{idVendor}=="0451", ATTR{idProduct}=="d012", TAG+="uaccess", MODE="0660", GROUP="plugdev"
  '';
}
