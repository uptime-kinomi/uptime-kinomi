Updates
=======

kinomi updates itself with ``uptime-kinomi self-update``, without root and
without a package manager. It only installs releases that are signed with
the kinomi release keys, which are kept offline, and tests the new version
before it replaces anything.

Automatic weekly updates
------------------------

``install.sh`` and the Ansible roles enable a timer that updates kinomi
every Monday between 04:00 and 06:00. If the Pi was off at that time, the
update runs after the next boot. Each run:

#. runs ``self-update``; if there is no new release, it stops here;
#. checks your configuration with the new version, and goes back to the
   old version if that fails;
#. restarts kinomi, waits 15 seconds and checks that it is still running,
   and goes back to the old version if not.

So an update never leaves you with a broken kinomi. To see when the timer
runs next, or to update right now:

.. code-block:: sh

   systemctl --user list-timers uptime-kinomi-update.timer
   systemctl --user start uptime-kinomi-update

What happened at the last update:

.. code-block:: sh

   journalctl --user -u uptime-kinomi-update

.. code-block:: text

   updated 0.5.0 -> 0.6.0 (/home/pi/.local/bin/uptime-kinomi). The previous version is kept as /home/pi/.local/bin/uptime-kinomi.old.

A failed update shows up in ``systemctl --user --failed``.

To switch automatic updates off:

.. code-block:: sh

   systemctl --user disable --now uptime-kinomi-update.timer

Updating by hand
----------------

.. code-block:: sh

   uptime-kinomi self-update --check     # is there a new release? (exit code 3 if yes)
   uptime-kinomi self-update             # download, verify, test, replace
   uptime-kinomi check-config            # does my configuration work with it?
   systemctl --user restart uptime-kinomi

``self-update`` replaces the program file, but the running kinomi keeps
using the old one until you restart it.

What ``self-update`` checks
---------------------------

#. It downloads the checksum list ``SHA256SUMS`` and its signature from the
   latest release on GitHub, over HTTPS.
#. The signature must be valid for one of the two release keys built into
   kinomi, and must say which version it is for. The version must be newer
   than yours, so nobody can push an old version with known bugs onto your
   Pi.
#. It downloads the program for your processor and compares its checksum.
#. It runs the new program once (``version``) to make sure it works on your
   processor.
#. Only then does it replace the program, keeping the old one as
   ``uptime-kinomi.old``.

A release is only signed after the maintainer has checked it, on a computer
that is not connected to GitHub. Even someone who takes over the GitHub
project cannot publish an update that your kinomi accepts.

Going back
----------

.. code-block:: sh

   uptime-kinomi self-update --rollback
   systemctl --user restart uptime-kinomi

This puts ``uptime-kinomi.old`` back in place. To install a specific
version, including an older one:

.. code-block:: sh

   uptime-kinomi self-update --release 0.5.0 --allow-downgrade

Read the :doc:`changelog </reference/changelog>` before you go back more
than one version: kinomi is pre-1.0, and configuration details can change
between versions.

System-wide installation
------------------------

The service cannot write ``/usr/local/bin``, so an administrator updates:

.. code-block:: sh

   sudo uptime-kinomi self-update
   sudo -u kinomi uptime-kinomi check-config --config-dir /etc/uptime-kinomi
   sudo systemctl restart uptime-kinomi

``self-update`` refuses to replace programs in ``/usr/bin`` or ``/bin``:
those belong to the package manager.
