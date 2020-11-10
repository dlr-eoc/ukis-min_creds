from min_creds_client import CredentialService
import time
import signal
import os

# example usage
if __name__ == "__main__":
    cs = CredentialService("https://hopper.eoc.dlr.de:9992/", "test_token", ssl_verify=False)
    service_name = "sentinelhub"

    for i in range(15):
        try:
            cs._request("POST", "/get", json={
                "service": service_name
            }, timeout=0.3)
        except:
            pass

    with cs.credential_lease(service_name) as lease:
        print(lease.user)
        print(lease.password)
        print(lease.expires_on)

        overview = cs.list()
        assert overview["services"][service_name]["credentials_in_use"] > 0

        # comment in to test release of leases on Ctrl-C, ...
        #os.kill(os.getpid(), signal.SIGINT)

        time.sleep(5)

