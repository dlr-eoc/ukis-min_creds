import os
import signal
import time

from min_creds_client import CredentialService

URL_OF_MIN_CREDS_SERVER = "https://localhost:9992" # when using the provided example.config.yaml of the server
SERVICE_NAME = "sentinelhub"

# example usage
if __name__ == "__main__":
    cs = CredentialService(URL_OF_MIN_CREDS_SERVER, "test_token", ssl_verify=False)
    with cs.credential_lease(SERVICE_NAME) as lease:
        print(lease.user)
        print(lease.password)
        print(lease.expires_on)
        print(lease.wait_secs)

        overview = cs.list()
        assert overview["services"][SERVICE_NAME]["credentials_in_use"] > 0

        # comment out to test release of leases on Ctrl-C, ...
        # os.kill(os.getpid(), signal.SIGINT)

        time.sleep(5)
