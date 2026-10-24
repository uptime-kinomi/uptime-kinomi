# ansible-role-kinomi

Deploys uptime-kinomi as a systemd **user** service for `KINOMI_USER`
(SPEC.md §10.5, §10.6):

- creates `KINOMI_USER` if it does not exist (become);
- enables lingering, so the service starts at boot without a login;
- installs the binary to `~/.local/bin/uptime-kinomi`;
- copies the configuration to a staging directory, runs `check-config` on
  it, and only then replaces `~/.config/uptime-kinomi`;
- installs `contrib/systemd/user/uptime-kinomi.service` and (re)starts it.

The role uses `contrib/systemd/user/` from this repository, so keep it inside
`contrib/` (or copy that directory next to it).

## Variables

| Variable | Default | Meaning |
|---|---|---|
| `KINOMI_USER` | `kinomi` | User kinomi runs as. |
| `kinomi_binary` | `{{ playbook_dir }}/files/uptime-kinomi-armv6` | Binary for the target board. |
| `kinomi_config` | `{{ playbook_dir }}/files/config/` | Configuration directory (start from `contrib/examples/config`). |

## Become

If `KINOMI_USER` is the Ansible login user, nothing needs root, except
lingering on systems where `loginctl enable-linger` is not allowed for the
user itself. For any other user, files are copied, the user is created and
lingering is enabled with become; run with `-K` (`--ask-become-pass`) if
sudo needs a password.

## Example

`site.yml` next to `files/`:

```yaml
- name: Deploy uptime-kinomi
  hosts: monitoring
  roles:
    - role: ansible-role-kinomi
      vars:
        KINOMI_USER: kinomi
```

```bash
ansible-playbook -i pi1.example.org, -K site.yml
```

with `roles_path` (or `ANSIBLE_ROLES_PATH`) pointing at `contrib/`.
For a system-wide service, use `contrib/systemd/system/` instead.
