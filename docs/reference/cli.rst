Command line
============

.. code-block:: text

   uptime-kinomi [COMMAND] [--config-dir DIR]

``run`` (default)
   Run kinomi: checks, notifications, pages and the web server, until it
   gets ``SIGTERM`` or ``SIGINT`` (:kbd:`Ctrl+C`). Usually started by
   systemd.

``check-config``
   Validate the whole configuration directory and print the directories and
   monitors. Exit code 2 on errors. Warnings (e.g. readable secrets) do not
   fail.

``rebuild-state``
   Regenerate all state files from the log. Stop the service first
   (:doc:`/operations/logs-and-data`).

``gen-token``
   Print a new random push token.

``hash-password USER``
   Print an htpasswd line for ``USER``. With a terminal, a random password
   is generated and shown once; otherwise the password is read from the
   first line of stdin.

``self-update``
   Install the latest signed release (from 1.0 on: not a new major version
   without ``--release``; :doc:`/reference/stability`; :doc:`/operations/updates`). Options:

   ``--check``
      Only check; exit code 3 if a newer release exists, 0 if not.
   ``--release X.Y.Z``
      Install this release.
   ``--allow-downgrade``
      With ``--release``: allow an older or the same version.
   ``--rollback``
      Restore the version that the last update replaced.

``version`` (also ``-V``, ``--version``)
   Print the version and the platform, e.g. ``uptime-kinomi 0.6.0
   (arm-unknown-linux-musleabihf)``.

``help`` (also ``-h``, ``--help``)
   Print the usage.

``--config-dir DIR`` (or ``--config-dir=DIR``) selects the configuration
directory; without it, ``$KINOMI_CONFIG_DIR``, then
``~/.config/uptime-kinomi`` if it exists, then ``/etc/uptime-kinomi``.

Exit codes
----------

.. list-table::
   :widths: 15 85

   * - 0
     - Success.
   * - 1
     - Runtime error (e.g. the port is in use, a download failed).
   * - 2
     - Configuration or usage error.
   * - 3
     - ``self-update --check``: an update is available.
