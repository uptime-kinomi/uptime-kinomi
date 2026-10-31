Installation with Ansible
=========================

With Ansible, your kinomi configuration lives on your laptop (ideally in
Git), and one command installs or updates it on one or many Pis. kinomi
ships two Ansible roles for this:

.. list-table::
   :header-rows: 1
   :widths: 25 75

   * - Role
     - Use it when
   * - ``ansible-role-user``
     - Ansible logs in as the user who should run kinomi, e.g. ``pi``. No
       root, no ``become``.
   * - ``ansible-role-root``
     - Ansible logs in as root. The role installs the needed packages and
       creates a user ``kinomi`` (or another name) that kinomi runs as.

Both roles do the same on the Pi:

* enable lingering, so kinomi starts at boot;
* if kinomi is not installed yet: download the latest release for the
  Pi's processor, check its signature and checksum, and install it to
  ``~/.local/bin``. An installed kinomi is never replaced by the role;
  updates are done by the weekly update timer (:doc:`/operations/updates`);
* send your configuration directory, check it with ``check-config`` on the
  Pi **before** it goes live, and only then replace
  ``~/.config/uptime-kinomi`` with it, if anything changed. Files you
  deleted or renamed are deleted on the Pi too. A configuration with errors
  never reaches the running service;
* install the systemd user service and the weekly update timer, and
  restart kinomi when the configuration changed.

What you need
-------------

* On your laptop: Ansible 2.15 or newer (``pipx install ansible-core``, or
  ``sudo apt install ansible``) and Git.
* SSH access to the Pi with a key (``ssh-copy-id pi@raspberrypi.local``).
* For ``ansible-role-user``: ``minisign`` on the Pi for the first
  installation (``sudo apt install minisign``). ``ansible-role-root``
  installs it itself.

Step 1: get the roles
---------------------

The roles are in the ``contrib/`` directory of the kinomi repository. They
use the systemd units next to them, so keep the directory as it is:

.. code-block:: sh

   git clone https://github.com/uptime-kinomi/uptime-kinomi.git ~/src/uptime-kinomi

Step 2: create your project
---------------------------

Your project holds the playbook, the inventory and your configuration:

.. code-block:: text

   my-kinomi/
   ├── ansible.cfg
   ├── inventory.ini
   ├── site.yml
   └── files/
       └── config/            your kinomi configuration
           ├── kinomi.toml
           ├── monitors/
           ├── notifications/
           └── pages/

Start the configuration from the complete example:

.. code-block:: sh

   mkdir -p ~/my-kinomi/files
   cd ~/my-kinomi
   cp -r ~/src/uptime-kinomi/contrib/examples/config files/config
   git init

``ansible.cfg`` tells Ansible where the roles are:

.. code-block:: ini
   :caption: ansible.cfg

   [defaults]
   roles_path = ~/src/uptime-kinomi/contrib
   inventory = inventory.ini

``inventory.ini`` lists your Pis and the user to log in as:

.. code-block:: ini
   :caption: inventory.ini

   [monitoring]
   raspberrypi.local ansible_user=pi

``site.yml`` applies the role:

.. code-block:: yaml
   :caption: site.yml

   - name: Deploy uptime-kinomi
     hosts: monitoring
     roles:
       - role: ansible-role-user

Step 3: adapt the configuration
-------------------------------

Edit the files in ``files/config/``: in ``kinomi.toml`` at least
``hostnames`` (the names under which you open the page) and ``timezone``,
then your monitors, notifiers and pages. The rest of this documentation
explains all of them; :doc:`/getting-started/first-steps` is a good start.

If you have kinomi on your laptop too, check the directory there first. It
is the same check the role runs on the Pi:

.. code-block:: sh

   uptime-kinomi check-config --config-dir files/config

.. important::

   Keep ``data_dir``, ``log_dir`` and ``output_dir`` **outside** the
   configuration directory (the defaults are). The role replaces the
   configuration directory as a whole, and stops if one of them is inside
   it.

Step 4: run the playbook
------------------------

.. code-block:: sh

   ansible-playbook site.yml

The first run installs kinomi. Later runs only change something when your
configuration changed: then the new configuration is checked, put in place,
and kinomi is restarted. Run it as often as you like.

If ``check-config`` finds an error, the play fails with kinomi's message,
and the Pi keeps running the previous configuration.

Secrets with Ansible Vault
--------------------------

Push tokens, notification tokens and passwords should not be in Git in
plain text. Encrypt each file that contains one with Ansible Vault, e.g.
the push monitor with its token:

.. code-block:: sh

   ansible-vault encrypt files/config/monitors/backup.toml
   ansible-playbook site.yml --ask-vault-pass

The roles decrypt Vault files while packing the configuration, so the Pi
gets the plain file and kinomi reads it as usual. (``ansible-vault edit``
opens an encrypted file for changes.) On the Pi, the whole
configuration directory is readable only by the user kinomi runs as.

Several Pis
-----------

Each Pi usually monitors different things, so give each its own
configuration directory with ``kinomi_config``:

.. code-block:: ini
   :caption: inventory.ini

   [monitoring]
   pi-home.local    ansible_user=pi  kinomi_config="{{ playbook_dir }}/files/pi-home/"
   pi-office.local  ansible_user=pi  kinomi_config="{{ playbook_dir }}/files/pi-office/"

To let the two Pis watch each other, see :doc:`/operations/two-pis`.

Logging in as root: ``ansible-role-root``
-----------------------------------------

If Ansible logs in as root, use the other role. It installs ``curl``,
``wget``, ``minisign`` and ``sudo``, creates the user ``KINOMI_USER``
(default ``kinomi``), and then does everything above for that user. kinomi
itself still runs as that user, never as root.

.. code-block:: yaml
   :caption: site.yml

   - name: Deploy uptime-kinomi
     hosts: monitoring
     roles:
       - role: ansible-role-root
         vars:
           KINOMI_USER: kinomi

.. code-block:: ini
   :caption: inventory.ini

   [monitoring]
   raspberrypi.local ansible_user=root

Variables
---------

.. list-table::
   :header-rows: 1
   :widths: 25 30 45

   * - Variable
     - Default
     - Meaning
   * - ``kinomi_config``
     - ``{{ playbook_dir }}/files/config/``
     - The configuration directory on your laptop.
   * - ``kinomi_auto_update``
     - ``true``
     - Enable the weekly update timer. With ``false``, update with
       ``uptime-kinomi self-update`` on the Pi.
   * - ``KINOMI_USER``
     - ``kinomi``
     - Only ``ansible-role-root``: the user kinomi runs as.

What the role does not do
-------------------------

* It does not update an installed kinomi. That is the job of the weekly
  timer, which also checks your configuration with the new version and
  rolls back if needed. To update right away: ``ssh pi@raspberrypi.local
  systemctl --user start uptime-kinomi-update``.
* It does not touch logs and state. Removing a monitor from the
  configuration keeps its history on the Pi.
* It does not set up a reverse proxy; see :doc:`/operations/reverse-proxy`.
