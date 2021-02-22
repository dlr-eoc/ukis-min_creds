import os
import signal
import time

from min_creds_client import CredentialService

# example usage
if __name__ == "__main__":
    cs = CredentialService("URL_OF_MIN_CREDS_SERVER", "test_token", ssl_verify=False)
    service_name = "NAME_OF_SERVICE"
    with cs.credential_lease(service_name) as lease:
        print(lease.user)
        print(lease.password)
        print(lease.expires_on)

        overview = cs.list()
        assert overview["services"][service_name]["credentials_in_use"] > 0

        # comment out to test release of leases on Ctrl-C, ...
        # os.kill(os.getpid(), signal.SIGINT)

        time.sleep(5)
