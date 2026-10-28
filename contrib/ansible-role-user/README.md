# ansible-role-user

Deploys uptime-kinomi as a systemd **user** service of the **login user**
(SPEC.md §10.5, §10.6). No become, no root. To create a dedicated user and
install packages, use [`ansible-role-root`](../ansible-role-root/) instead.

- enables lingering, so the service starts at boot without a login;
- if `~/.local/bin/uptime-kinomi` does not exist: downloads the latest
  release for the CPU, verifies the minisign signature of `SHA256SUMS` and
  the checksum, and installs it. An existing binary is never touched;
  updates are done by `self-update`;
- sends the configuration as one archive (Ansible Vault files decrypted,
  symlinks followed), unpacks it into an emptied staging directory, runs
  `check-config` there, and only then replaces
  `~/.config/uptime-kinomi` with it, if anything differs. Files removed or
  renamed in your configuration disappear on the target too. The role stops
  if `data_dir`, `log_dir` or `output_dir` is inside the configuration
  directory (they would be deleted). The target needs `tar`, `gzip` and
  `diff`;
- installs the units from `contrib/systemd/user/`, (re)starts the service
  and enables the weekly self-update timer, unless `kinomi_auto_update: false`.

**Needs on the target:** `minisign` (`apt install minisign`) for the first
installation. On systems where `loginctl enable-linger` is not allowed
without root, enable lingering once with `sudo loginctl enable-linger USER`.

The role uses `contrib/systemd/user/` from this repository, so keep it inside
`contrib/` (or copy that directory next to it).

## Variables

| Variable | Default | Meaning |
|---|---|---|
| `kinomi_config` | `{{ playbook_dir }}/files/config/` | Configuration directory (start from `contrib/examples/config`). |
| `kinomi_auto_update` | `true` | Enable the weekly self-update timer. With `false`, update with `uptime-kinomi self-update`. |

## Example

`site.yml` next to `files/config/`:

```yaml
- name: Deploy uptime-kinomi
  hosts: monitoring
  roles:
    - role: ansible-role-user
```

```bash
ANSIBLE_ROLES_PATH=contrib ansible-playbook -i kinomi@pi1.example.org, site.yml
```
