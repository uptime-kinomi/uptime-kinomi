Telegram
========

kinomi sends Telegram messages through a bot that you create. The bot can
write to you, to a group, or to a channel.

Step 1: create a bot
--------------------

#. In Telegram, open a chat with **@BotFather** and send ``/newbot``.
#. Choose a name and a user name for the bot (the user name must end in
   ``bot``).
#. BotFather answers with a **token** like
   ``123456789:AAH...``. Store it in a file only you can read:

   .. code-block:: sh

      echo '123456789:AAH...' > ~/.config/uptime-kinomi/secrets/telegram.token
      chmod 600 ~/.config/uptime-kinomi/secrets/telegram.token

Step 2: find the chat ID
------------------------

A bot may only write to chats that talked to it first.

* **To yourself:** open the chat with your new bot and send it any message.
* **To a group:** add the bot to the group and send a message there.

Then open this address in a browser, with your token:

.. code-block:: text

   https://api.telegram.org/bot<TOKEN>/getUpdates

Look for ``"chat":{"id":...}`` in the answer. Your own chat ID is a
positive number; group IDs are negative, like ``-1001234567890``. For a
public channel, add the bot as an administrator and use ``"@channelname"``
as chat ID.

Step 3: create the notifier
---------------------------

.. literalinclude:: /examples/catalog/notifications/telegram.toml
   :language: toml
   :caption: notifications/telegram.toml

The chat ID is written in quotes, as text, even when it is a number.

Keys
----

.. list-table::
   :header-rows: 1
   :widths: 30 20 50

   * - Key
     - Default
     - Meaning
   * - ``bot_token`` / ``bot_token_file``
     - (one required)
     - The token from @BotFather.
   * - ``chat_id``
     - (required)
     - Numeric chat ID or ``"@channelname"``, as a string.
