Hi {{ name }},

The password for your Collaboration Tool account ({{ email }}) was just changed. Every device that was signed in has been signed out.
{% if claude_disconnected %}
Claude was disconnected too. If you use it, connect it again from More, then Connect Claude.
{% endif %}
If you did this, there is nothing else to do.

If you did not, someone else can read your email. Secure your email account first, then reset your password again here:

{{ forgot_url }}

Then tell an owner of each organisation you belong to.
