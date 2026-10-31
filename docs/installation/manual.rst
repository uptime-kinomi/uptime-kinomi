Manual installation
===================

This page installs kinomi step by step, by hand. You see exactly what goes
where, and you can adapt every step: no systemd, other directories, or a
system-wide service with its own user.

The first part needs **no root**. Only the
:ref:`system-wide installation <system-wide>` at the end does.

Step 1: download the program
----------------------------

Find out which file you need with ``uname -m`` and the table in
:ref:`which-binary`. On a Raspberry Pi 1 that is ``uptime-kinomi-armv6``;
replace it in all commands below if yours is different.

.. code-block:: sh

   cd ~
   base=https://github.com/uptime-kinomi/uptime-kinomi/releases/latest/download
   curl -fsSLO $base/uptime-kinomi-armv6
   curl -fsSLO $base/SHA256SUMS
   curl -fsSLO $base/SHA256SUMS.minisig

``SHA256SUMS`` lists a checksum for every program file, and
``SHA256SUMS.minisig`` is the signature of that list.

Step 2: check the download
--------------------------

The signature proves that the checksum list was signed with the kinomi
release key, which is kept offline, not on GitHub. The checksum then proves
that your program file is the one in the list.

.. code-block:: sh

   sudo apt install minisign          # once; or ask your administrator
   minisign -Vm SHA256SUMS -P RWQWavktNED2FrnDC8yLcpUPBKv4JeUoguh4zMF2wCBxwRJKVs3vRZu3
   sha256sum --ignore-missing -c SHA256SUMS

Both must succeed:

.. code-block:: text

   Signature and comment signature verified
   Trusted comment: uptime-kinomi 0.7.0
   uptime-kinomi-armv6: OK

The key above is the primary release key (ID ``16F640342DF96A16``). If a
release is ever signed with the backup key (ID ``6AD1615F6A7966B2``), use
``RWSyZnlqX2HRasxyKYKZQUmcQYpm5e9zHsDUFWTgvWDerhQj+DccOfUH`` instead. Both
keys are also in the ``keys/`` directory of the repository.

.. note::

   No minisign and no way to install it? :doc:`install-script` can check
   the signature with OpenSSL 3 instead, which Raspberry Pi OS already has.
   If you skip the signature, the checksum alone only protects against a
   broken download, not against a replaced one.

Optionally, with the GitHub CLI you can also check where the file was
built: ``gh attestation verify uptime-kinomi-armv6 --repo
uptime-kinomi/uptime-kinomi``.

Step 3: install the program
---------------------------

.. code-block:: sh

   mkdir -p ~/.local/bin
   install -m 755 uptime-kinomi-armv6 ~/.local/bin/uptime-kinomi
   rm uptime-kinomi-armv6 SHA256SUMS SHA256SUMS.minisig
   ~/.local/bin/uptime-kinomi version

.. code-block:: text

   uptime-kinomi 0.7.0 (arm-unknown-linux-musleabihf)

Raspberry Pi OS and Debian put ``~/.local/bin`` into your ``PATH`` at login
if it exists. Log out and in again once, then ``uptime-kinomi`` works
without the path.

Keeping the program in a directory you own is what lets
``uptime-kinomi self-update`` update it later without root.

Step 4: write a configuration
-----------------------------

The configuration is a directory with one small text file per thing. Create
the directory and its subdirectories:

.. code-block:: sh

   mkdir -p ~/.config/uptime-kinomi/monitors ~/.config/uptime-kinomi/pages
   chmod 700 ~/.config/uptime-kinomi
   cd ~/.config/uptime-kinomi

Create ``kinomi.toml`` with an editor, e.g. ``nano kinomi.toml``:

.. literalinclude:: /examples/manual/kinomi.toml
   :language: toml
   :caption: ~/.config/uptime-kinomi/kinomi.toml

``hostnames`` lists the names under which the status page may be opened:
the Pi's host name (``hostname``) and its IP address (``hostname -I``).
Requests with any other name are refused with "421 Misdirected Request".
``timezone`` is used to show times on the pages; ``timedatectl`` shows
yours.

One monitor, ``monitors/website.toml``:

.. literalinclude:: /examples/manual/monitors/website.toml
   :language: toml
   :caption: ~/.config/uptime-kinomi/monitors/website.toml

And one status page, ``pages/home.toml``:

.. literalinclude:: /examples/manual/pages/home.toml
   :language: toml
   :caption: ~/.config/uptime-kinomi/pages/home.toml

Now let kinomi check the configuration:

.. code-block:: sh

   uptime-kinomi check-config

.. code-block:: text

   config dir: /home/pi/.config/uptime-kinomi
   data dir:   /home/pi/.local/state/uptime-kinomi
   log dir:    /home/pi/.local/state/uptime-kinomi/log
   output dir: /run/user/1000/uptime-kinomi/www
   monitors:   1
     website                  http  every    1m  https://www.example.org/
   pages:      1 (home)
   notifiers:  0
   incidents:  0
   maintenance windows: 0
   configuration OK

A typo is reported with the file and the line, and nothing starts until it
is fixed. :doc:`/getting-started/concepts` explains the files in more
detail.

Step 5: try it
--------------

Start kinomi in the foreground:

.. code-block:: sh

   uptime-kinomi run

Open ``http://raspberrypi.local:8080/`` in a browser. After a few seconds
your monitor is ✓ UP. Stop kinomi with :kbd:`Ctrl+C`.

Step 6: start it automatically
------------------------------

With systemd (Raspberry Pi OS, Debian, Ubuntu, ...), install the user
service:

.. code-block:: sh

   mkdir -p ~/.config/systemd/user
   cd ~/.config/systemd/user
   repo=https://raw.githubusercontent.com/uptime-kinomi/uptime-kinomi/main/contrib/systemd/user
   curl -fsSLO $repo/uptime-kinomi.service
   systemctl --user daemon-reload
   systemctl --user enable --now uptime-kinomi

This is the service file:

.. literalinclude:: /../contrib/systemd/user/uptime-kinomi.service
   :language: ini
   :caption: ~/.config/systemd/user/uptime-kinomi.service

A user service normally stops when you log out and only starts when you log
in. *Lingering* changes that: your services start at boot and keep running.
Enable it once:

.. code-block:: sh

   loginctl enable-linger

If that fails with "Access denied", an administrator has to run ``sudo
loginctl enable-linger pi`` once.

Step 7 (optional): weekly updates
---------------------------------

The update timer runs ``uptime-kinomi self-update`` every Monday morning,
checks your configuration with the new version, restarts the service, and
goes back to the old version if anything fails
(:doc:`/operations/updates`):

.. code-block:: sh

   cd ~/.config/systemd/user
   curl -fsSLO $repo/uptime-kinomi-update.service
   curl -fsSLO $repo/uptime-kinomi-update.timer
   systemctl --user daemon-reload
   systemctl --user enable --now uptime-kinomi-update.timer

Without systemd
---------------

Start kinomi at boot from your crontab (``crontab -e``):

.. code-block:: text

   @reboot ~/.local/bin/uptime-kinomi run

Without systemd there is usually no ``/run/user/<uid>``, so kinomi writes
its pages into ``~/.local/state/uptime-kinomi/www`` on the SD card, about
every minute. To spare the card, either render less often or put the pages
into memory, in ``kinomi.toml``:

.. code-block:: toml

   [pages]
   render_interval = "5m"                      # render every 5 minutes
   # or: output_dir = "/dev/shm/uptime-kinomi-pi"   # a directory in memory

Updates: run ``uptime-kinomi self-update`` from time to time (or from
``crontab``), then restart kinomi.

.. _system-wide:

System-wide installation with its own user
------------------------------------------

On a server you may prefer a classic system service: the program in
``/usr/local/bin``, the configuration in ``/etc/uptime-kinomi``, and a
dedicated user ``kinomi`` that owns nothing else. kinomi still runs
without root; root is only needed to set it up. The shipped system unit
is hardened (no capabilities, read-only system, no access to home
directories).

.. code-block:: sh

   # the user and the program (after steps 1 and 2)
   sudo useradd --system --home-dir /var/lib/uptime-kinomi --shell /usr/sbin/nologin kinomi
   sudo install -m 755 uptime-kinomi-armv6 /usr/local/bin/uptime-kinomi

   # the configuration: owned by root, readable by the group kinomi
   sudo mkdir -p /etc/uptime-kinomi/monitors /etc/uptime-kinomi/pages
   sudo nano /etc/uptime-kinomi/kinomi.toml          # see below
   sudo chown -R root:kinomi /etc/uptime-kinomi
   sudo chmod -R g+rX,o-rwx /etc/uptime-kinomi

   # the service
   sudo curl -fsSLo /etc/systemd/system/uptime-kinomi.service \
     https://raw.githubusercontent.com/uptime-kinomi/uptime-kinomi/main/contrib/systemd/system/uptime-kinomi.service
   sudo systemctl daemon-reload
   sudo -u kinomi uptime-kinomi check-config --config-dir /etc/uptime-kinomi
   sudo systemctl enable --now uptime-kinomi

systemd creates ``/var/lib/uptime-kinomi``, ``/var/log/uptime-kinomi`` and
``/run/uptime-kinomi`` for the service. Set the same directories in
``kinomi.toml``, so that a manual run as the ``kinomi`` user uses them too:

.. literalinclude:: /examples/system/kinomi.toml
   :language: toml
   :caption: /etc/uptime-kinomi/kinomi.toml

Monitors and pages go into ``/etc/uptime-kinomi/monitors/`` and
``/etc/uptime-kinomi/pages/`` as usual. After a change:

.. code-block:: sh

   sudo -u kinomi uptime-kinomi check-config --config-dir /etc/uptime-kinomi
   sudo systemctl restart uptime-kinomi

**Updates:** ``/usr/local/bin`` is read-only for the service, so update as
an administrator:

.. code-block:: sh

   sudo uptime-kinomi self-update
   sudo systemctl restart uptime-kinomi

The pages in ``/run/uptime-kinomi/www`` belong to the group ``kinomi``. A
web server that should serve them directly (mode B in
:doc:`/operations/reverse-proxy`) must be in that group: ``sudo usermod -aG
kinomi www-data``.

Building from source
--------------------

For a processor without a release binary, or to run your own changes,
build kinomi with Rust (1.85 or newer, from https://rustup.rs):

.. code-block:: sh

   git clone https://github.com/uptime-kinomi/uptime-kinomi.git
   cd uptime-kinomi
   cargo build --release
   install -m 755 target/release/uptime-kinomi ~/.local/bin/uptime-kinomi

Building on a Raspberry Pi 1 takes hours; build on a faster computer
instead, e.g. with `cross <https://github.com/cross-rs/cross>`_:
``cross build --release --target arm-unknown-linux-musleabihf``.
``self-update`` replaces a self-built binary with the next signed release.
