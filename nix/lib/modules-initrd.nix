{
  lib,
  runCommand,
  cpio,
  xz,
  gawk,
  gnugrep,
  kmod,
  linux-firmware,
}:

{
  name,
  kernel,
  modules,
  excludedModules ? [ ],
  firmware ? [ ],
  extraCommands ? "",
}:

let
  version = kernel.modDirVersion;
  source = "${kernel.modules}/lib/modules/${version}";
  anyOf = patterns: lib.optionalString (patterns != [ ]) "^kernel/(${lib.concatStringsSep "|" patterns})";
in
runCommand "${name}.cpio.xz"
  {
    nativeBuildInputs = [
      cpio
      xz
      gawk
      gnugrep
      kmod
    ];
    included = anyOf modules;
    excluded = anyOf excludedModules;
  }
  ''
    STAGING=$PWD/staging
    MODULES=$STAGING/lib/modules/${version}
    FIRMWARE=${linux-firmware}/lib/firmware
    mkdir -p "$MODULES" "$STAGING/lib/firmware"

    for pattern in ${lib.escapeShellArgs modules}; do
      if ! grep -qE "^kernel/($pattern)" ${source}/modules.dep; then
        echo "no module of linux ${version} matches '$pattern'" >&2
        exit 1
      fi
    done

    cp --no-preserve=mode ${source}/modules.{order,builtin,builtin.modinfo} "$MODULES/"
    awk -F': ?' '
      BEGIN { included = ENVIRON["included"]; excluded = ENVIRON["excluded"] }
      $1 ~ included && (excluded == "" || $1 !~ excluded) {
        print $1
        n = split($2, deps, " ")
        for (i = 1; i <= n; i++) print deps[i]
      }
    ' ${source}/modules.dep \
      | sort -u \
      | (cd ${source} && xargs -r cp --parents --no-preserve=mode -t "$MODULES")
    chmod -R u+w "$MODULES"
    find "$MODULES" -name '*.ko.xz' -exec xz -d {} +

    declare -A staged=()
    stage_firmware() {
      local file=$1 hash
      if [ -e "$STAGING/lib/firmware/$file" ]; then
        return
      fi
      mkdir -p "$STAGING/lib/firmware/$(dirname "$file")"
      hash=$(sha256sum "$FIRMWARE/$file" | cut -d' ' -f1)
      if [ -n "''${staged[$hash]:-}" ]; then
        ln -sr "$STAGING/lib/firmware/''${staged[$hash]}" "$STAGING/lib/firmware/$file"
      else
        cp -L --no-preserve=mode "$FIRMWARE/$file" "$STAGING/lib/firmware/$file"
        staged[$hash]=$file
      fi
    }

    cd "$FIRMWARE"
    shopt -s nullglob
    for pattern in ${lib.escapeShellArgs firmware}; do
      matches=($pattern)
      if [ ! -e "''${matches[0]:-}" ]; then
        echo "no firmware in ${linux-firmware.name} matches '$pattern'" >&2
        exit 1
      fi
      while read -r file; do
        stage_firmware "$file"
      done < <(find -L "''${matches[@]}" -type f)
    done
    shopt -u nullglob

    ${extraCommands}

    cd "$STAGING"
    find . | cpio -o -H newc 2>/dev/null | xz --check=crc32 -9e --threads=0 --block-size=48MiB > "$out"
  ''
