Installation with install.sh
============================

The install script does everything in one go: it downloads kinomi for your
processor, checks that the download really comes from the kinomi project,
writes a starter configuration, and starts kinomi as a service that also
runs after a reboot. Then it updates kinomi once a week.

It needs no root and changes nothing outside your home directory.

Before you start
----------------

* Log in to the Pi (or whatever computer kinomi should run on) **as the
  user who should run kinomi**, for example with ``ssh pi@raspberrypi.local``.
  Do not use ``sudo`` and do not log in as root: the script refuses to run
  as root.
* The system needs ``curl`` or ``wget`` (both are installed on Raspberry Pi
  OS) and, to check the signature, either ``minisign`` or OpenSSL 3.
  Raspberry Pi OS 12 (bookworm) and newer, Debian 12 and newer and Ubuntu
  22.04 and newer have OpenSSL 3, so there is nothing to install.

Step 1: decide who may see the status page
------------------------------------------

kinomi has a small web server for its status pages. By default it only
answers on the Pi itself (``127.0.0.1``). That is right if you will put a
web server like nginx in front of it later (see
:doc:`/operations/reverse-proxy`).

At home you usually want to open the page from your laptop or phone. Then
let kinomi listen on all network interfaces with ``--listen
0.0.0.0:8080``. The examples below do that.

Step 2: run the script
----------------------

.. code-block:: sh

   curl -fsSL https://raw.githubusercontent.com/uptime-kinomi/uptime-kinomi/main/contrib/install.sh | sh -s -- --listen 0.0.0.0:8080

Without ``--listen``, the command is just ``... | sh``.

.. tip::

   Don't like piping a script from the internet into a shell? Download it,
   read it, then run it:

   .. code-block:: sh

      curl -fsSLO https://raw.githubusercontent.com/uptime-kinomi/uptime-kinomi/main/contrib/install.sh
      less install.sh
      sh install.sh --listen 0.0.0.0:8080

The output on a Raspberry Pi 1 looks like this:

.. code-block:: text

   ==> Downloading uptime-kinomi-armv6
   ==> Checking the signature and the checksum
       uptime-kinomi 0.6.0 (arm-unknown-linux-musleabihf), signed release 0.6.0
   ==> Creating a starter configuration in /home/pi/.config/uptime-kinomi
   ==> Installing to /home/pi/.local/bin
   ==> Installing the systemd user service

   uptime-kinomi is installed.
     binary:        /home/pi/.local/bin/uptime-kinomi
     configuration: /home/pi/.config/uptime-kinomi
     service:       systemctl --user status uptime-kinomi
     logs:          journalctl --user -u uptime-kinomi
     updates:       weekly (systemctl --user list-timers uptime-kinomi-update.timer)
     status page:   http://0.0.0.0:8080/
     pages:         /run/user/1000/uptime-kinomi/www
     data:          /home/pi/.local/state/uptime-kinomi
     check log:     /home/pi/.local/state/uptime-kinomi/log

   Next: add your monitors to /home/pi/.config/uptime-kinomi/monitors/, then
     uptime-kinomi check-config && systemctl --user restart uptime-kinomi
   Updates: installed automatically once a week; now: systemctl --user start uptime-kinomi-update

If the signature or the checksum does not match, the script stops before
anything is installed.

What the script did
-------------------

#. It picked the program file for your processor (:ref:`which-binary`) and
   downloaded it with the list of checksums and its signature.
#. It checked the signature against the two kinomi release keys built into
   the script, then the checksum of the program, and ran the program once to
   see that it works on your processor.
#. It created a **starter configuration** in ``~/.config/uptime-kinomi``:

   * ``kinomi.toml``: the server address, the names under which the page
     may be opened (your Pi's host name and IP addresses), and your time
     zone, all detected automatically;
   * ``monitors/example-web.toml``: checks that https://example.org/ is
     reachable;
   * ``monitors/internet-dns.toml``: checks that name resolution works;
   * ``pages/home.toml``: a public status page with all monitors.

   An existing configuration is never overwritten.
#. It installed the program as ``~/.local/bin/uptime-kinomi``.
#. It installed and started two systemd **user** units: the service
   ``uptime-kinomi`` and the timer ``uptime-kinomi-update.timer``, which
   updates kinomi every Monday morning (see :doc:`/operations/updates`).
#. It enabled *lingering* for your user, so that your services run without
   you being logged in and start at boot.

Step 3: open the status page
----------------------------

Open ``http://raspberrypi.local:8080/`` in a browser on your laptop or phone
(use your Pi's host name or IP address; the ``0.0.0.0`` in the script's
output means "all addresses of this computer"). You see a page called
*Status* with two monitors. Within a minute they turn green: ✓ **UP**.

If the page does not open, see :ref:`page-does-not-open`.

Step 4: check the service
-------------------------

.. code-block:: sh

   systemctl --user status uptime-kinomi

shows ``active (running)``. kinomi writes every check into its log:

.. code-block:: sh

   tail -f ~/.local/state/uptime-kinomi/log/$(date -u +%F).log

.. code-block:: text

   2026-11-01T10:00:02Z event=START version=0.6.0 monitors=2
   2026-11-01T10:00:05Z monitor=example-web result=UP latency_ms=234 cert_expires=2026-12-25 msg="HTTP 200"
   2026-11-01T10:00:07Z monitor=internet-dns result=UP latency_ms=12 msg="2 A records"

Press :kbd:`Ctrl+C` to stop watching. The service keeps running.

If the script said that lingering could not be enabled
-------------------------------------------------------

On some systems only an administrator may enable lingering. The script
then ends with:

.. code-block:: text

   To keep it running after you log out and to start it at boot, run once:
     sudo loginctl enable-linger pi

Ask someone with ``sudo`` rights to run that command once. Until then,
kinomi stops when you log out.

If ``uptime-kinomi`` is "command not found"
-------------------------------------------

The program is in ``~/.local/bin``. Raspberry Pi OS and Debian add that
directory to your ``PATH`` at login if it exists, so **log out and in
again** once. Until then, use the full path ``~/.local/bin/uptime-kinomi``.

Options
-------

Options go after ``sh -s --`` when you pipe the script, or after
``sh install.sh`` when you downloaded it:

``--listen ADDRESS``
   The address of the status page server in the new configuration, e.g.
   ``0.0.0.0:8080`` (whole network) or ``127.0.0.1:8080`` (only this
   computer, the default). It has no effect on an existing configuration:
   change ``listen`` in ``kinomi.toml`` instead.

``--version X.Y.Z``
   Install this release instead of the latest one.

``--no-service``
   Install only the program and the configuration, no systemd service. The
   script then tells you how to start kinomi by hand or from ``crontab``.

``--no-auto-update``
   Do not enable the weekly update timer. You then update with
   ``uptime-kinomi self-update`` (see :doc:`/operations/updates`).

``--uninstall``
   Stop and remove the program and the services. Your configuration, logs
   and data stay where they are; delete ``~/.config/uptime-kinomi`` and
   ``~/.local/state/uptime-kinomi`` yourself if you want them gone.

Running the script again
------------------------

You can run the script again at any time, for example to repair an
installation. It keeps your configuration, checks it with the newly
downloaded version **before** replacing anything, and restarts the service.
If your configuration does not work with the new version, it stops and
changes nothing.

No minisign and no OpenSSL 3?
-----------------------------

On older systems the script cannot check the signature and stops:

.. code-block:: text

   error: cannot check the release signature: install minisign (or OpenSSL 3), or set KINOMI_ALLOW_UNSIGNED=1 to rely on HTTPS and checksums only

Install minisign (``sudo apt install minisign``) and run the script again.
If you cannot, ``KINOMI_ALLOW_UNSIGNED=1`` skips the signature check; then
only HTTPS and the checksum protect the download:

.. code-block:: sh

   curl -fsSL https://raw.githubusercontent.com/uptime-kinomi/uptime-kinomi/main/contrib/install.sh | KINOMI_ALLOW_UNSIGNED=1 sh

Next step
---------

Continue with :doc:`/getting-started/first-steps`: add your own monitors and
get notified on your phone.
