ntfy
====

`ntfy <https://ntfy.sh>`_ sends push messages to your phone. The public
server ``ntfy.sh`` is free and needs no account; you can also run your own.

With ntfy.sh
------------

#. Install the **ntfy** app (Android: Google Play or F-Droid; iOS: App
   Store).
#. Choose a **topic name**. Topics on ntfy.sh are public: anyone who knows
   the name can read your messages and send you fake ones. Make it long and
   random, for example with ``echo kinomi-$(head -c 8 /dev/urandom | od
   -An -tx1 | tr -d ' \n')``.
#. In the app, tap **+**, enter the topic, and subscribe.
#. Create the notifier:

.. literalinclude:: /examples/catalog/notifications/phone.toml
   :language: toml
   :caption: notifications/phone.toml

You can also read the messages in a browser at ``https://ntfy.sh/<topic>``.

Messages have a title (``My blog is DOWN``), the text, a coloured tag
(🔴 down and reminder, 🟢 up, 🟡 warn) and a priority: 4 ("high") for
``down`` and ``reminder``, 3 ("default") otherwise. The app can make high
priority messages louder.

Your own ntfy server
--------------------

With access control on your own server, create a user with write access to
the topic and an access token for it (``ntfy token add kinomi``), then:

.. code-block:: sh

   echo 'tk_...' > ~/.config/uptime-kinomi/secrets/ntfy.token
   chmod 600 ~/.config/uptime-kinomi/secrets/ntfy.token

.. literalinclude:: /examples/catalog/notifications/ntfy-selfhosted.toml
   :language: toml
   :caption: notifications/ntfy-selfhosted.toml

Keys
----

.. list-table::
   :header-rows: 1
   :widths: 25 25 50

   * - Key
     - Default
     - Meaning
   * - ``url``
     - ``"https://ntfy.sh"``
     - The ntfy server.
   * - ``topic``
     - (required)
     - The topic: ``A-Z a-z 0-9 _ -``, at most 64 characters.
   * - ``token`` / ``token_file``
     - none
     - Access token for a server with access control.
   * - ``priority``
     - 4 for down/reminder, 3 otherwise
     - 1 (min) to 5 (max), for all events of this notifier.
