Backup and moving to a new Pi
=============================

What to back up
---------------

.. list-table::
   :widths: 40 60

   * - ``~/.config/uptime-kinomi/``
     - Your configuration, passwords and tokens. Essential (unless it is in
       Git and deployed with Ansible).
   * - ``~/.local/state/uptime-kinomi/``
     - The log and the state files: the history of your monitors. Without
       it, kinomi starts from scratch, with no uptime history.

The pages need no backup; they are generated. The program itself can be
downloaded again.

The files can be copied while kinomi runs. For a perfectly consistent copy,
stop it for the moment of the copy:

.. code-block:: sh

   systemctl --user stop uptime-kinomi
   tar -czf ~/kinomi-backup-$(date +%F).tar.gz -C ~ .config/uptime-kinomi .local/state/uptime-kinomi
   systemctl --user start uptime-kinomi

The archive contains your tokens and password hashes: store it somewhere
safe.

Moving to a new Pi
------------------

#. Install kinomi on the new Pi (:doc:`/installation/index`). This creates
   a starter configuration, which you replace in the next steps.
#. Stop kinomi on both:

   .. code-block:: sh

      systemctl --user stop uptime-kinomi

#. Copy the configuration and the data from the old Pi to the new one
   (run on the new Pi):

   .. code-block:: sh

      rm -rf ~/.config/uptime-kinomi ~/.local/state/uptime-kinomi
      scp -r pi@old-pi.local:.config/uptime-kinomi ~/.config/
      scp -r pi@old-pi.local:.local/state/uptime-kinomi ~/.local/state/

#. Adapt ``hostnames`` in ``kinomi.toml`` if the new Pi has a different
   name or address, then check and start:

   .. code-block:: sh

      uptime-kinomi check-config && systemctl --user start uptime-kinomi

#. Update the addresses where others reach kinomi: push URLs in your
   scripts (only the host part; the tokens stay the same), the reverse
   proxy, bookmarks.
#. Switch the old Pi off for good:
   ``systemctl --user disable --now uptime-kinomi``.

The configuration directory can be copied as it is: relative paths in it
are relative to the directory itself. If a different user runs kinomi on
the new Pi, this works the same.

Uninstalling
------------

.. code-block:: sh

   curl -fsSL https://raw.githubusercontent.com/uptime-kinomi/uptime-kinomi/main/contrib/install.sh | sh -s -- --uninstall

removes the program and the services. Then delete the data if you no longer
want it:

.. code-block:: sh

   rm -rf ~/.config/uptime-kinomi ~/.local/state/uptime-kinomi
