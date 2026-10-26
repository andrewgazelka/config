# Distraction sites, sinkholed for the whole machine.
{ lib, ... }:
let
  dns = [
    "discord.com"
    "discord.gg"
    "twitch.tv"
    "netflix.com"
  ];

  sinkhole = lib.concatMapStrings (host: ''
    127.0.0.1 ${host}
    127.0.0.1 www.${host}
    ::1 ${host}
    ::1 www.${host}
  '') dns;
in
{
  system.activationScripts.extraActivation.text = ''
    cat > /etc/hosts <<'EOF'
    127.0.0.1 localhost
    255.255.255.255 broadcasthost
    ::1 localhost

    ${sinkhole}
    EOF
  '';
}
